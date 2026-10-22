"""Offline exercise of actual example runners; never connects to a robot."""

import argparse
import asyncio
import importlib
import itertools
import json
import sys
from pathlib import Path
from types import ModuleType, SimpleNamespace
from unittest.mock import AsyncMock

import numpy as np
import pytest


def response(payload, code=0):
    return {"data": {"header": {"status": {"code": code}}, "data": payload}}


@pytest.fixture
def examples(monkeypatch):
    native = ModuleType("unitree_webrtc_rs")
    native.RTC_TOPIC = {
        name: f"fake/{name}"
        for name in (
            "SPORT_MOD",
            "MOTION_SWITCHER",
            "LOW_STATE",
            "LF_SPORT_MOD_STATE",
            "ULIDAR_ARRAY",
            "ULIDAR_SWITCH",
        )
    }
    native.SPORT_CMD = {"GetState": 1034, "StandUp": 1004, "StandDown": 1005}
    native.WebRTCConnectionMethod = SimpleNamespace(LocalSTA=2)
    connections = []

    class Connection:
        def __init__(self, method, ip, aes_128_key=None):
            self.ip = ip
            self.key = aes_128_key
            self.isConnected = False
            self.connect = AsyncMock(side_effect=self.open)
            self.disconnect = AsyncMock(side_effect=self.close)
            self.requests = []
            self.responses = []
            self.height = 0.07
            self.rpy = [0.0, 0.0, 0.0]
            self.subscribe = self.subscribe_state
            self.unsubscribe = lambda topic: self.unsubscribed.append(topic)
            self.unsubscribed = []
            self.video_frame = np.zeros((2, 3, 3), dtype=np.uint8)
            self.video_switches = []
            self.lidar_points = np.ones((4, 3), dtype=np.float32)
            self.publications = []
            self.datachannel = SimpleNamespace(
                pub_sub=self,
                disableTrafficSaving=AsyncMock(return_value=True),
                set_decoder=lambda **kwargs: None,
                heartbeat_running=True,
                last_heartbeat_age_s=0,
            )
            self.video = SimpleNamespace(
                on_frame=self.on_frame, switchVideoChannel=self.video_switches.append
            )
            # No audio methods: integration must not touch audio by default.
            self.audio = SimpleNamespace()
            connections.append(self)

        def open(self):
            self.isConnected = True

        def close(self):
            self.isConnected = False

        async def publish_request_new(self, topic, request, timeout=None):
            self.requests.append((topic, request))
            api = request["api_id"]
            if api == native.SPORT_CMD["StandUp"]:
                self.height = 0.33
            if api == native.SPORT_CMD["StandDown"]:
                self.height = 0.07
            if self.responses:
                return self.responses.pop(0)
            return response({"name": "normal"})

        def publish_without_callback(self, topic, data):
            self.publications.append((topic, data))

        def subscribe_state(self, topic, callback):
            data = {
                "imu_state": {"rpy": self.rpy},
                "motor_state": [{}] * 12,
                "bms_state": {},
                "mode": 0,
                "body_height": self.height,
            }
            if topic == native.RTC_TOPIC["ULIDAR_ARRAY"]:
                data = {"data": {"points": self.lidar_points}}
            callback({"data": data})

        def on_frame(self, callback):
            callback(self.video_frame)

    native.UnitreeWebRTCConnection = Connection
    monkeypatch.setitem(sys.modules, "unitree_webrtc_rs", native)
    monkeypatch.syspath_prepend(str(Path(__file__).resolve().parents[1] / "examples"))
    names = (
        "example_support",
        "connect",
        "lidar_stream",
        "lidar_test",
        "lowstate_test",
        "sport_mode_state",
        "video_stream",
        "sport_mode",
        "integration",
        "action_validation",
    )
    modules = {}
    for name in names:
        monkeypatch.delitem(sys.modules, name, raising=False)
        modules[name] = importlib.import_module(name)
    sleep = AsyncMock()
    monkeypatch.setattr(asyncio, "sleep", sleep)
    args = argparse.Namespace(
        ip="offline",
        duration=0.001,
        aes_128_key=None,
        aes_128_key_file=None,
        allow_motion=False,
        posture="cycle",
        include_audio=False,
        headless=True,
        output=None,
    )
    parse_sport_args = modules["sport_mode"].parse_args
    for module in modules.values():
        if hasattr(module, "parse_args"):
            monkeypatch.setattr(module, "parse_args", lambda: args)
    # Bounded clock removes wall-clock waits in the video loop.
    ticks = itertools.count(step=0.01)
    monkeypatch.setattr(
        modules["video_stream"], "time", SimpleNamespace(monotonic=lambda: next(ticks))
    )
    return SimpleNamespace(
        modules=modules,
        native=native,
        connections=connections,
        args=args,
        parse_sport_args=parse_sport_args,
    )


def run_main(examples, name):
    asyncio.run(examples.modules[name].main())
    conn = examples.connections[-1]
    conn.disconnect.assert_awaited_once()
    return conn


@pytest.mark.parametrize(
    "name",
    (
        "connect",
        "lidar_stream",
        "lidar_test",
        "lowstate_test",
        "sport_mode_state",
        "video_stream",
        "sport_mode",
        "integration",
        "action_validation",
    ),
)
def test_non_audio_example_default_is_bounded_and_cleans_up(examples, name):
    conn = run_main(examples, name)
    assert not conn.isConnected
    assert all(
        request["api_id"] == examples.native.SPORT_CMD["GetState"]
        for _, request in conn.requests
    )
    assert conn.key is None


def test_explicit_motion_only_stands_up_and_down(examples):
    examples.args.allow_motion = True
    conn = run_main(examples, "sport_mode")
    assert conn.requests == [
        (
            examples.native.RTC_TOPIC["SPORT_MOD"],
            {"api_id": examples.native.SPORT_CMD["GetState"]},
        ),
        (examples.native.RTC_TOPIC["MOTION_SWITCHER"], {"api_id": 1001}),
        (
            examples.native.RTC_TOPIC["SPORT_MOD"],
            {"api_id": examples.native.SPORT_CMD["StandUp"]},
        ),
        (
            examples.native.RTC_TOPIC["SPORT_MOD"],
            {"api_id": examples.native.SPORT_CMD["StandDown"]},
        ),
    ]
    assert conn.height == 0.07


def override_connection(examples, monkeypatch, change):
    support = examples.modules["example_support"]
    factory = support.UnitreeWebRTCConnection

    def create(*args, **kwargs):
        conn = factory(*args, **kwargs)
        change(conn)
        return conn

    monkeypatch.setattr(support, "UnitreeWebRTCConnection", create)


@pytest.mark.parametrize(
    "reply", [None, {}, response({}, 1), response({}, False), response({}, "0")]
)
def test_failed_query_never_actuates_and_disconnects(examples, monkeypatch, reply):
    examples.args.allow_motion = True
    override_connection(
        examples, monkeypatch, lambda conn: conn.responses.append(reply)
    )
    with pytest.raises(ValueError):
        run_main(examples, "sport_mode")
    conn = examples.connections[-1]
    conn.disconnect.assert_awaited_once()
    assert len(conn.requests) == 1


@pytest.mark.parametrize(
    "payload", [{"name": "ai"}, {"name": "unknown"}, {}, "not JSON", []]
)
def test_invalid_mode_does_not_switch_mode_or_send_motion(
    examples, monkeypatch, payload
):
    examples.args.allow_motion = True
    override_connection(
        examples,
        monkeypatch,
        lambda conn: conn.responses.extend([response({}), response(payload)]),
    )
    with pytest.raises(ValueError):
        run_main(examples, "sport_mode")
    conn = examples.connections[-1]
    conn.disconnect.assert_awaited_once()
    assert len(conn.requests) == 2


@pytest.mark.parametrize(
    "rpy", [None, [True, 0, 0], [float("nan"), 0, 0], [0.7, 0, 0], [0, 0]]
)
def test_missing_or_unsafe_orientation_blocks_motion(examples, monkeypatch, rpy):
    examples.args.allow_motion = True
    override_connection(examples, monkeypatch, lambda conn: setattr(conn, "rpy", rpy))
    with pytest.raises(ValueError):
        run_main(examples, "sport_mode")
    assert len(examples.connections[-1].requests) == 2
    examples.connections[-1].disconnect.assert_awaited_once()


@pytest.mark.parametrize("failed_index", (0, 1))
def test_failed_posture_command_stops_sequence(examples, monkeypatch, failed_index):
    examples.args.allow_motion = True
    replies = (
        [response({}), response({"name": "normal"})]
        + [response({})] * failed_index
        + [response({}, 1)]
    )
    override_connection(
        examples, monkeypatch, lambda conn: conn.responses.extend(replies)
    )
    with pytest.raises(ValueError):
        run_main(examples, "sport_mode")
    assert len(examples.connections[-1].requests) == failed_index + 3
    examples.connections[-1].disconnect.assert_awaited_once()


@pytest.mark.parametrize(
    "payload", [{"name": "normal"}, json.dumps({"name": "normal"})]
)
def test_normal_mode_dict_or_json_is_supported(examples, monkeypatch, payload):
    examples.args.allow_motion = True
    override_connection(
        examples,
        monkeypatch,
        lambda conn: conn.responses.extend([response({}), response(payload)]),
    )
    assert run_main(examples, "sport_mode").height == 0.07


@pytest.mark.parametrize("name", ("lidar_stream", "lowstate_test", "sport_mode_state"))
def test_missing_stream_is_failure_not_success(examples, monkeypatch, name):
    override_connection(
        examples,
        monkeypatch,
        lambda conn: setattr(conn, "subscribe", lambda *args: None),
    )
    with pytest.raises(RuntimeError):
        run_main(examples, name)
    examples.connections[-1].disconnect.assert_awaited_once()


@pytest.mark.parametrize(
    "points",
    [
        np.zeros((4, 3), dtype=np.float64),
        np.zeros((0, 3), dtype=np.float32),
        np.zeros((3, 4), dtype=np.float32),
        np.full((1, 3), np.nan, dtype=np.float32),
    ],
)
def test_invalid_lidar_contract_fails_and_unsubscribes(examples, monkeypatch, points):
    override_connection(
        examples, monkeypatch, lambda conn: setattr(conn, "lidar_points", points)
    )
    with pytest.raises(RuntimeError):
        run_main(examples, "lidar_stream")
    conn = examples.connections[-1]
    assert conn.unsubscribed == [examples.native.RTC_TOPIC["ULIDAR_ARRAY"]]
    conn.disconnect.assert_awaited_once()


@pytest.mark.parametrize(
    "frame",
    [
        np.zeros((2, 3), dtype=np.uint8),
        np.zeros((2, 3, 3), dtype=np.float32),
        np.zeros((0, 3, 3), dtype=np.uint8),
    ],
)
def test_invalid_video_contract_fails_and_disables_channel(
    examples, monkeypatch, frame
):
    override_connection(
        examples, monkeypatch, lambda conn: setattr(conn, "video_frame", frame)
    )
    with pytest.raises(RuntimeError):
        run_main(examples, "video_stream")
    conn = examples.connections[-1]
    assert conn.video_switches == [True, False]
    conn.disconnect.assert_awaited_once()


def test_connect_failure_is_cleaned_up(examples, monkeypatch):
    override_connection(
        examples,
        monkeypatch,
        lambda conn: setattr(
            conn.connect, "side_effect", RuntimeError("private credential")
        ),
    )
    with pytest.raises(RuntimeError):
        run_main(examples, "connect")
    examples.connections[-1].disconnect.assert_awaited_once()


def test_disconnect_failure_does_not_mask_primary_failure(
    examples, monkeypatch, caplog
):
    def fail(conn):
        conn.connect.side_effect = RuntimeError("primary failure")
        conn.disconnect.side_effect = RuntimeError("private credential")

    override_connection(examples, monkeypatch, fail)
    with pytest.raises(RuntimeError, match="primary failure"):
        run_main(examples, "connect")
    assert "Disconnect failed" in caplog.text
    assert "private credential" not in caplog.text


def test_disconnect_failure_is_not_reported_as_success(examples, monkeypatch):
    override_connection(
        examples,
        monkeypatch,
        lambda conn: setattr(
            conn.disconnect, "side_effect", RuntimeError("private credential")
        ),
    )
    with pytest.raises(RuntimeError, match="Disconnect failed"):
        run_main(examples, "connect")


@pytest.mark.parametrize("value", ["0", "-1", "nan", "inf", "3601"])
def test_duration_rejects_unbounded_or_invalid_values(examples, value):
    with pytest.raises(argparse.ArgumentTypeError):
        examples.modules["example_support"].positive_seconds(value)


def test_ip_has_no_hardcoded_device_default(examples, monkeypatch):
    monkeypatch.delenv("ROBOT_IP", raising=False)
    parser = examples.modules["example_support"].argument_parser("offline")
    with pytest.raises(SystemExit):
        parser.parse_args([])


def test_key_env_and_file_precedence(examples, monkeypatch, tmp_path):
    support = examples.modules["example_support"]
    monkeypatch.setenv("UNITREE_AES_128_KEY", " env-key ")
    assert support.resolve_aes_key(examples.args) == "env-key"
    path = tmp_path / "key"
    path.write_text("file-key\n")
    examples.args.aes_128_key_file = str(path)
    assert support.resolve_aes_key(examples.args) == "file-key"
    examples.args.aes_128_key = " argv-key "
    assert support.resolve_aes_key(examples.args) == "argv-key"


def test_cli_failure_redacts_secret_and_exits_nonzero(examples, caplog):
    async def failing():
        raise RuntimeError("private credential")

    with pytest.raises(SystemExit) as error:
        examples.modules["example_support"].run(failing)
    assert error.value.code == 1
    assert "private credential" not in caplog.text


def test_cli_reports_unsupported_posture_mode_without_peer_text(
    examples, monkeypatch, caplog
):
    examples.args.allow_motion = True
    override_connection(
        examples,
        monkeypatch,
        lambda conn: conn.responses.extend(
            [response({}), response({"name": "private credential"})]
        ),
    )
    with pytest.raises(SystemExit) as error:
        examples.modules["example_support"].run(examples.modules["sport_mode"].main)
    assert error.value.code == 1
    assert "reason=unsupported_posture_mode" in caplog.text
    assert "private credential" not in caplog.text
    assert len(examples.connections[-1].requests) == 2
    examples.connections[-1].disconnect.assert_awaited_once()


def test_cli_distinguishes_lidar_validation_failure_from_transport_failure(
    examples, monkeypatch, caplog
):
    def suppress_lidar(conn):
        subscribe = conn.subscribe

        def receive(topic, callback):
            if topic != examples.native.RTC_TOPIC["ULIDAR_ARRAY"]:
                subscribe(topic, callback)

        conn.subscribe = receive

    override_connection(examples, monkeypatch, suppress_lidar)
    with pytest.raises(SystemExit) as error:
        examples.modules["example_support"].run(examples.modules["lidar_stream"].main)
    assert error.value.code == 1
    assert "reason=lidar_stream_missing_invalid_or_stale" in caplog.text
    examples.connections[-1].disconnect.assert_awaited_once()


def test_stale_state_blocks_success(examples, monkeypatch):
    module = examples.modules["lowstate_test"]
    clock = iter((0.0, 2.0))
    monkeypatch.setattr(module, "time", SimpleNamespace(monotonic=lambda: next(clock)))
    conn = examples.native.UnitreeWebRTCConnection(2, ip="offline")
    with pytest.raises(RuntimeError, match="No valid state"):
        asyncio.run(
            module.collect_state(
                conn, examples.native.RTC_TOPIC["LOW_STATE"], 0.001, ("imu_state",)
            )
        )
    assert conn.unsubscribed == [examples.native.RTC_TOPIC["LOW_STATE"]]


@pytest.mark.parametrize("height", (True, float("nan"), -1.0, 2.0, 0.33))
def test_invalid_or_already_standing_baseline_blocks_motion(
    examples, monkeypatch, height
):
    examples.args.allow_motion = True
    override_connection(
        examples, monkeypatch, lambda conn: setattr(conn, "height", height)
    )
    with pytest.raises(ValueError):
        run_main(examples, "sport_mode")
    assert len(examples.connections[-1].requests) == 2
    examples.connections[-1].disconnect.assert_awaited_once()


def test_ack_without_posture_change_is_not_physical_success(examples, monkeypatch):
    examples.args.allow_motion = True

    def no_movement(conn):
        request = conn.publish_request_new

        async def unchanged(*args, **kwargs):
            reply = await request(*args, **kwargs)
            conn.height = 0.07
            return reply

        conn.publish_request_new = unchanged

    override_connection(examples, monkeypatch, no_movement)
    with pytest.raises(RuntimeError, match="telemetry did not confirm"):
        run_main(examples, "sport_mode")
    assert len(examples.connections[-1].requests) == 4
    assert (
        examples.connections[-1].requests[-1][1]["api_id"]
        == examples.native.SPORT_CMD["StandDown"]
    )
    examples.connections[-1].disconnect.assert_awaited_once()


@pytest.mark.parametrize("age", (None, True, 4, "0"))
def test_missing_or_stale_heartbeat_blocks_motion(examples, monkeypatch, age):
    examples.args.allow_motion = True
    override_connection(
        examples,
        monkeypatch,
        lambda conn: setattr(conn.datachannel, "last_heartbeat_age_s", age),
    )
    with pytest.raises(ValueError, match="heartbeat"):
        run_main(examples, "sport_mode")
    assert len(examples.connections[-1].requests) == 2
    examples.connections[-1].disconnect.assert_awaited_once()


def test_unsafe_orientation_after_standup_does_not_send_blind_lowering(
    examples, monkeypatch
):
    examples.args.allow_motion = True

    def becomes_unsafe(conn):
        request = conn.publish_request_new

        async def unsafe(topic, payload, **kwargs):
            reply = await request(topic, payload, **kwargs)
            if payload["api_id"] == examples.native.SPORT_CMD["StandUp"]:
                conn.rpy = [0.8, 0, 0]
            return reply

        conn.publish_request_new = unsafe

    override_connection(examples, monkeypatch, becomes_unsafe)
    with pytest.raises(ValueError, match="Unsafe orientation"):
        run_main(examples, "sport_mode")
    assert len(examples.connections[-1].requests) == 3
    examples.connections[-1].disconnect.assert_awaited_once()


@pytest.mark.parametrize("address", ("", " ", "host name", "\n"))
def test_empty_or_whitespace_address_is_rejected(examples, address):
    with pytest.raises(argparse.ArgumentTypeError):
        examples.modules["example_support"].robot_address(address)


@pytest.mark.parametrize("name", ("lidar_stream", "video_stream"))
def test_early_stopped_media_stream_does_not_report_success(
    examples, monkeypatch, name
):
    module = examples.modules[name]
    ticks = iter((0.0, 2.0) if name == "lidar_stream" else (0.0, 0.0, 2.0, 2.0))
    monkeypatch.setattr(module, "time", SimpleNamespace(monotonic=lambda: next(ticks)))
    with pytest.raises(RuntimeError, match="No valid"):
        run_main(examples, name)
    conn = examples.connections[-1]
    conn.disconnect.assert_awaited_once()
    if name == "video_stream":
        assert conn.video_switches == [True, False]
    else:
        assert conn.unsubscribed == [examples.native.RTC_TOPIC["ULIDAR_ARRAY"]]


def test_explicit_single_standup_leaves_standing(examples, capsys):
    examples.args.allow_motion = True
    examples.args.posture = "stand-up"
    conn = run_main(examples, "sport_mode")
    assert conn.height == 0.33
    assert [request["api_id"] for _, request in conn.requests] == [
        examples.native.SPORT_CMD["GetState"],
        1001,
        examples.native.SPORT_CMD["StandUp"],
    ]
    assert "robot will remain standing" in capsys.readouterr().out


def test_explicit_single_standdown_finishes_lowered(examples, monkeypatch):
    examples.args.allow_motion = True
    examples.args.posture = "stand-down"
    override_connection(
        examples, monkeypatch, lambda conn: setattr(conn, "height", 0.33)
    )
    conn = run_main(examples, "sport_mode")
    assert conn.height == 0.07
    assert [request["api_id"] for _, request in conn.requests] == [
        examples.native.SPORT_CMD["GetState"],
        1001,
        examples.native.SPORT_CMD["StandDown"],
    ]


@pytest.mark.parametrize("posture", ("stand-up", "stand-down"))
def test_single_posture_cli_requires_explicit_motion_flag(examples, posture):
    with pytest.raises(SystemExit) as error:
        examples.parse_sport_args(["--ip", "offline", "--posture", posture])
    assert error.value.code == 2
    assert examples.connections == []


@pytest.mark.parametrize("posture", ("cycle", "stand-up", "stand-down"))
def test_posture_cli_with_explicit_flag_is_accepted(examples, posture):
    args = examples.parse_sport_args(
        ["--ip", "offline", "--allow-motion", "--posture", posture]
    )
    assert args.allow_motion
    assert args.posture == posture


def test_single_standdown_requires_standing_baseline(examples):
    examples.args.allow_motion = True
    examples.args.posture = "stand-down"
    with pytest.raises(ValueError, match="Start standing"):
        run_main(examples, "sport_mode")
    assert len(examples.connections[-1].requests) == 2
    examples.connections[-1].disconnect.assert_awaited_once()


@pytest.mark.parametrize("payload", ({"name": "mcf"}, json.dumps({"name": "mcf"})))
@pytest.mark.parametrize(
    "posture, api_id, baseline, expected_height",
    (
        ("stand-up", 1004, 0.07, 0.33),
        ("stand-down", 1005, 0.33, 0.07),
    ),
)
def test_mcf_single_posture_uses_only_requested_api_without_mode_switch(
    examples, monkeypatch, payload, posture, api_id, baseline, expected_height
):
    examples.args.allow_motion = True
    examples.args.posture = posture

    def configure(conn):
        conn.height = baseline
        conn.responses.extend([response({}), response(payload)])

    override_connection(examples, monkeypatch, configure)
    conn = run_main(examples, "sport_mode")
    assert conn.requests == [
        (examples.native.RTC_TOPIC["SPORT_MOD"], {"api_id": 1034}),
        (examples.native.RTC_TOPIC["MOTION_SWITCHER"], {"api_id": 1001}),
        (examples.native.RTC_TOPIC["SPORT_MOD"], {"api_id": api_id}),
    ]
    assert conn.height == expected_height
    assert conn.publications == []
