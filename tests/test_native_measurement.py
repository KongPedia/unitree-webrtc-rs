"""Exercise the bounded offline measurement CLI against its actual native extension."""

import json
import subprocess
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/measure-native.py"


@pytest.mark.parametrize(
    "arguments",
    [
        ["--iterations", "0"],
        ["--iterations", "1001"],
        ["--batches", "0"],
        ["--batches", "17"],
        ["--callback-cycles", "0"],
        ["--callback-cycles", "1001"],
        ["--warmup", "-1"],
        ["--warmup", "101"],
        ["--iterations", "1000", "--batches", "16"],
    ],
)
def test_invalid_measurement_bounds_fail_before_native_import(arguments):
    # -S removes site packages: reaching the extension import would fail differently.
    result = subprocess.run(
        [sys.executable, "-S", str(SCRIPT), *arguments],
        capture_output=True,
        text=True,
        timeout=10,
        check=False,
    )
    assert result.returncode == 2, result.stderr
    assert "bounded workload" in result.stderr
    assert "ModuleNotFoundError" not in result.stderr
    assert result.stdout == ""


def test_small_native_measurement_checks_outcomes_and_releases_every_callback_owner():
    result = subprocess.run(
        [
            sys.executable,
            str(SCRIPT),
            "--iterations",
            "3",
            "--batches",
            "2",
            "--callback-cycles",
            "4",
            "--warmup",
            "1",
        ],
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    report = json.loads(result.stdout)
    assert report["native"]["loader"] == "ExtensionFileLoader"
    assert len(report["native"]["sha256"]) == 64
    assert report["connection"] == {
        "ip": "127.0.0.1",
        "connected": False,
        "connect_called": False,
        "disconnect_warmed_before_baseline": True,
    }
    assert report["json_outcomes"] == {"RuntimeError:not_open": 12, "ValueError:budget": 18}
    assert report["callback_cycles"]["samples"] == 8
    assert report["callback_cycles"]["retained_before_clear"] == 8
    assert report["callback_cycles"]["released_after_clear"] == 8
    assert report["callback_cycles"]["registrations"] == 16
    assert report["callback_cycles"]["callback_invocations"] == 0
    assert {row["name"]: row["expected_outcome"] for row in report["json_workloads"]} == {
        "accepted_shared_dag": "RuntimeError:not_open",
        "accepted_wide_list": "RuntimeError:not_open",
        "rejected_shared_dag": "ValueError:budget",
        "rejected_wide_list": "ValueError:budget",
        "rejected_utf8_bytes": "ValueError:budget",
    }
    for workload in [*report["json_workloads"], report["callback_cycles"]]:
        assert workload["samples"] == (8 if workload["name"] == "callback_register_clear_owner_release" else 6)
        assert workload["unit"] == "operations"
        latency = workload["latency_ns"]
        assert 0 <= latency["p50"] <= latency["p95"] <= latency["p99"] <= latency["max"]
    assert len(report["resources"]["batches"]) == 2
    for snapshot in [report["resources"]["baseline"], report["resources"]["final"]]:
        assert snapshot["rss_bytes"] is None or snapshot["rss_bytes"] > 0
        assert snapshot["native_threads"] is None or snapshot["native_threads"] > 0
        if snapshot["source"] is not None and snapshot["native_threads"] is not None:
            # Constructor workers plus the warmed runtime are native OS threads,
            # not just threading.active_count() or the first ps process row.
            assert snapshot["native_threads"] > 1
    assert report["resources"]["peak_rss_bytes"] is None or report["resources"]["peak_rss_bytes"] > 0
    assert "scripts/measure-native.py" in report["source_sha256"]
