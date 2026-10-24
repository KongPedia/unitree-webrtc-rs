#!/usr/bin/env python3
"""Measure bounded disconnected native FFI preflight and callback-owner release; never connect."""

import argparse
import asyncio
import gc
import hashlib
import importlib
import json
import math
import os
import platform
import subprocess
import sys
import time
import weakref
from importlib.machinery import ExtensionFileLoader
from importlib.metadata import version
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--iterations", type=int, default=100, help="JSON calls per fixture per batch (1-1000)")
    parser.add_argument("--batches", type=int, default=4, help="resource-observed batches (1-16)")
    parser.add_argument("--callback-cycles", type=int, default=100, help="register/clear cycles per batch (1-1000)")
    parser.add_argument("--warmup", type=int, default=10, help="untimed operations per workload (0-100)")
    args = parser.parse_args(argv)
    for name, low, high in (
        ("iterations", 1, 1000),
        ("batches", 1, 16),
        ("callback_cycles", 1, 1000),
        ("warmup", 0, 100),
    ):
        if not low <= getattr(args, name) <= high:
            parser.error(f"bounded workload: {name.replace('_', '-')} must be {low}-{high}")
    if args.batches * max(args.iterations, args.callback_cycles) > 8000:
        parser.error("bounded workload: batches times iterations/callback-cycles must not exceed 8000")
    return args


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def command(*args):
    try:
        return subprocess.check_output(args, text=True, stderr=subprocess.DEVNULL, timeout=2).strip()
    except (OSError, subprocess.SubprocessError):
        return None


def resource_snapshot():
    rss, threads, source = None, None, None
    status = Path("/proc/self/status")
    if sys.platform.startswith("linux"):
        try:
            fields = dict(line.split(":", 1) for line in status.read_text().splitlines() if ":" in line)
            rss = int(fields["VmRSS"].split()[0]) * 1024
            threads = int(fields["Threads"].strip())
            source = "/proc/self/status VmRSS (KiB), Threads"
        except (OSError, KeyError, ValueError):
            pass
    elif sys.platform == "darwin":
        pid = str(os.getpid())
        rss_text = command("ps", "-o", "rss=", "-p", pid)
        thread_text = command("ps", "-M", "-p", pid)
        if rss_text is not None:
            try:
                rss = int(rss_text) * 1024
            except ValueError:
                pass
        if thread_text is not None:
            rows = [line.split() for line in thread_text.splitlines()]
            # Darwin omits USER on continuation thread rows, moving PID to
            # the first field. Count both layouts, not only the process row.
            count = sum(pid in row[:2] for row in rows)
            threads = count or None
        source = "Darwin ps rss (KiB), ps -M thread rows"
    return {"rss_bytes": rss, "native_threads": threads, "source": source}


def peak_rss():
    if sys.platform != "darwin" and not sys.platform.startswith("linux"):
        return None
    import resource

    value = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return int(value if sys.platform == "darwin" else value * 1024)


def shared_dag(depth):
    value = None
    for _ in range(depth):
        value = [value, value]
    return value


def fixtures():
    # Repeated references are noncyclic: tiny Python DAGs expand during conversion.
    return [
        ("accepted_shared_dag", shared_dag(8), "RuntimeError:not_open", {"dag_depth": 8, "expanded_values": 511}),
        ("accepted_wide_list", [None] * 128, "RuntimeError:not_open", {"list_items": 128}),
        ("rejected_shared_dag", shared_dag(15), "ValueError:budget", {"dag_depth": 15, "expanded_values": 65535}),
        ("rejected_wide_list", [None] * 32768, "ValueError:budget", {"list_items": 32768}),
        ("rejected_utf8_bytes", "é" * 524289, "ValueError:budget", {"characters": 524289, "utf8_bytes": 1048578}),
    ]


def check_json(publish, payload, expected):
    try:
        publish("offline/measurement", payload)
    except (RuntimeError, ValueError) as error:
        required = RuntimeError if expected == "RuntimeError:not_open" else ValueError
        fragment = "not open" if required is RuntimeError else "budget"
        if type(error) is not required or fragment not in str(error):
            raise AssertionError(f"unexpected native failure category; expected {expected}") from error
    else:
        raise AssertionError(f"disconnected operation unexpectedly succeeded; expected {expected}")
    return expected


class CallbackOwner:
    def __init__(self, connection, invocations):
        self.connection = connection
        self.invocations = invocations

    def callback(self, _frame):
        self.invocations[0] += 1


def callback_cycle(connection, invocations):
    owner = CallbackOwner(connection, invocations)
    retained = weakref.ref(owner)
    connection.video.on_frame(owner.callback)
    connection.audio.on_audio(owner.callback)
    del owner
    if retained() is None:
        raise AssertionError("native registration did not retain the bound callback owner")
    connection.clear_callbacks()
    if retained() is not None:
        raise AssertionError("clear_callbacks retained a captured connection owner")


def summary(name, samples, elapsed_ns):
    ordered = sorted(samples)
    return {
        "name": name,
        "samples": len(samples),
        "unit": "operations",
        "latency_ns": {
            key: ordered[math.ceil(len(ordered) * rank) - 1]
            for key, rank in (("p50", 0.5), ("p95", 0.95), ("p99", 0.99), ("max", 1.0))
        },
        "elapsed_seconds": elapsed_ns / 1e9,
        "operations_per_second": len(samples) * 1e9 / elapsed_ns,
    }


async def measure(args):
    # Import only after CLI bounds pass. Never replace actual native execution with mocks.
    sdk = importlib.import_module("unitree_webrtc_rs")
    binary = importlib.import_module("unitree_webrtc_rs.unitree_webrtc_rs")
    if (
        not isinstance(binary.__loader__, ExtensionFileLoader)
        or sdk.UnitreeWebRTCConnection is not binary.UnitreeWebRTCConnection
    ):
        raise RuntimeError("measurement requires the actual native extension loader")
    connection = sdk.UnitreeWebRTCConnection(sdk.WebRTCConnectionMethod.LocalSTA, ip="127.0.0.1")
    workloads = fixtures()
    publish = connection.datachannel.pub_sub.publish_without_callback
    durations = {name: [] for name, *_ in workloads}
    elapsed = {name: 0 for name, *_ in workloads}
    callback_samples, callback_elapsed = [], 0
    invocations = [0]
    outcomes = {"RuntimeError:not_open": 0, "ValueError:budget": 0}
    try:
        # First actual async call initializes the shared native runtime before baseline.
        if await asyncio.wait_for(connection.disconnect(), timeout=5) != () or connection.is_connected:
            raise AssertionError("offline disconnect did not establish a disconnected baseline")
        for _, payload, expected, _ in workloads:
            for _ in range(args.warmup):
                check_json(publish, payload, expected)
        for _ in range(args.warmup):
            callback_cycle(connection, invocations)
        gc.collect()
        baseline = resource_snapshot()
        batches = []
        for batch in range(args.batches):
            before = resource_snapshot()
            for name, payload, expected, _ in workloads:
                batch_started = time.perf_counter_ns()
                for _ in range(args.iterations):
                    started = time.perf_counter_ns()
                    observed = check_json(publish, payload, expected)
                    durations[name].append(time.perf_counter_ns() - started)
                    outcomes[observed] += 1
                elapsed[name] += time.perf_counter_ns() - batch_started
            batch_started = time.perf_counter_ns()
            for _ in range(args.callback_cycles):
                started = time.perf_counter_ns()
                callback_cycle(connection, invocations)
                callback_samples.append(time.perf_counter_ns() - started)
            callback_elapsed += time.perf_counter_ns() - batch_started
            gc.collect()
            batches.append({"batch": batch + 1, "before": before, "after": resource_snapshot()})
        final = resource_snapshot()
        if connection.is_connected or invocations[0] != 0:
            raise AssertionError("measurement must remain disconnected without callback execution")
        cycles = args.callback_cycles * args.batches
        json_reports = [
            {**summary(name, durations[name], elapsed[name]), "expected_outcome": expected, "fixture": metadata}
            for name, _, expected, metadata in workloads
        ]
        sources = (
            "scripts/measure-native.py",
            "tests/test_native_measurement.py",
            "rust-toolchain.toml",
            "src/interface/utils.rs",
            "src/interface/callbacks.rs",
            "src/interface/bridges/connection.rs",
        )
        report = {
            "schema_version": 1,
            "scope": (
                "whole disconnected FFI JSON validation/preflight and bound callback "
                "registration/clear/owner-release operations"
            ),
            "latency_note": (
                "perf_counter_ns and Python outcome/weakref checks included; nearest-rank percentiles, "
                "not network/delivery/callback-execution latency"
            ),
            "limits": (
                "No connect/media enable/motion; no speedup baseline, real robot, soak, "
                "allocator-retention guarantee, retained-array or whole-process memory budget"
            ),
            "unix_seconds": time.time(),
            "python": sys.version,
            "platform": platform.platform(),
            "architecture": platform.machine(),
            "numpy_version": version("numpy"),
            "sdk_version": version("unitree-webrtc-rs"),
            "source_revision": command("git", "-C", str(ROOT), "rev-parse", "HEAD"),
            "source_sha256": {name: sha256(ROOT / name) for name in sources if (ROOT / name).is_file()},
            "native": {
                "loader": type(binary.__loader__).__name__,
                "file": binary.__file__,
                "sha256": sha256(binary.__file__),
            },
            "connection": {
                "ip": "127.0.0.1",
                "connected": False,
                "connect_called": False,
                "disconnect_warmed_before_baseline": True,
            },
            "workload": vars(args),
            "clock": {
                "implementation": time.get_clock_info("perf_counter").implementation,
                "resolution_seconds": time.get_clock_info("perf_counter").resolution,
                "unit": "nanoseconds",
            },
            "warmup": {
                "json_operations": len(workloads) * args.warmup,
                "callback_cycles": args.warmup,
                "included_in_samples": False,
            },
            "json_outcomes": outcomes,
            "json_workloads": json_reports,
            "callback_cycles": {
                **summary("callback_register_clear_owner_release", callback_samples, callback_elapsed),
                "retained_before_clear": cycles,
                "released_after_clear": cycles,
                "registrations": cycles * 2,
                "callback_invocations": invocations[0],
            },
            "resources": {
                "scope": (
                    "same fixed disconnected connection, warmed native runtime and all fixture objects "
                    "retained in baseline/final; sample storage grows during measurement; GC between "
                    "batches; unsupported observations null"
                ),
                "baseline": baseline,
                "batches": batches,
                "final": final,
                "peak_rss_bytes": peak_rss(),
                "peak_rss_scope": (
                    "process high-water mark including imports, warmup, fixtures and metadata, not current "
                    "RSS or native-only allocation"
                ),
                "interpretation": (
                    "allocator retention, thread scheduling and OS sampling affect observations; no "
                    "cross-platform RSS threshold or forever-no-leak claim"
                ),
            },
        }
        return report
    finally:
        connection.clear_callbacks()
        await asyncio.wait_for(connection.disconnect(), timeout=5)


def main(argv=None):
    args = arguments(argv)
    print(json.dumps(asyncio.run(measure(args)), indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
