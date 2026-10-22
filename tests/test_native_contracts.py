"""Offline contracts against the actual native extension, never a mocked module."""

import importlib
from importlib.machinery import ExtensionFileLoader
import subprocess
import sys

import pytest


@pytest.fixture(scope="module")
def native():
    module = importlib.import_module("unitree_webrtc_rs")
    binary = importlib.import_module("unitree_webrtc_rs.unitree_webrtc_rs")
    assert isinstance(binary.__loader__, ExtensionFileLoader)
    assert module.UnitreeWebRTCConnection is binary.UnitreeWebRTCConnection
    return module


@pytest.fixture(scope="module")
def connection(native):
    return native.UnitreeWebRTCConnection(
        native.WebRTCConnectionMethod.LocalSTA,
        ip="127.0.0.1",
        serialNumber="offline-fixture",
    )


def test_native_constructor_and_legacy_properties(native, connection):
    assert connection.serial_number == "offline-fixture"
    assert connection.connection_method == connection.connectionMethod
    assert connection.is_connected is connection.isConnected is False
    channel = connection.datachannel
    assert channel.ready_state == channel.readyState
    assert channel.is_open is channel.isOpen is False
    assert channel.heartbeat_running is channel.heartbeatRunning is False
    assert channel.last_heartbeat_age_s == channel.lastHeartbeatAgeS
    with pytest.raises(ValueError):
        native.UnitreeWebRTCConnection(-1)


def test_sport_commands_keep_canonical_wire_meanings(native):
    assert {
        name: native.SPORT_CMD[name]
        for name in ("StandUp", "StandDown", "Euler", "SwitchGait")
    } == {"StandUp": 1004, "StandDown": 1005, "Euler": 1007, "SwitchGait": 1011}
    assert "WALK" not in native.SPORT_CMD
    assert "RUN" not in native.SPORT_CMD


@pytest.mark.parametrize("value", [None, 42, {}])
@pytest.mark.parametrize("target", ["video", "audio", "topic"])
def test_callback_registration_rejects_noncallables(connection, target, value):
    register = {
        "video": connection.video.on_frame,
        "audio": connection.audio.on_audio,
        "topic": lambda callback: connection.datachannel.pub_sub.subscribe(
            "offline/test", callback
        ),
    }[target]
    with pytest.raises(TypeError, match="callable"):
        register(value)


def test_media_callback_registration_accepts_callables(connection):
    class Callback:
        def __call__(self, _frame):
            pass

    assert connection.video.on_frame(Callback()) is None
    assert connection.audio.on_audio(lambda _frame: None) is None


@pytest.mark.parametrize("media,method", [("video", "on_frame"), ("audio", "on_audio")])
def test_replacing_callback_allows_reentrant_python_finalizer(native, media, method):
    code = """
import importlib
import sys

native = importlib.import_module("unitree_webrtc_rs")
connection = native.UnitreeWebRTCConnection(
    native.WebRTCConnectionMethod.LocalSTA, ip="127.0.0.1"
)
register = getattr(getattr(connection, sys.argv[1]), sys.argv[2])
events = []

class Callback:
    def __call__(self, _frame):
        pass

    def __del__(self):
        register(lambda _frame: None)
        events.append("finalized")

register(Callback())
register(lambda _frame: None)
assert events == ["finalized"], events
"""
    subprocess.run(
        [sys.executable, "-c", code, media, method],
        check=True,
        timeout=10,
        capture_output=True,
        text=True,
    )


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "method", ["publish", "publish_request_new", "publish_without_callback"]
)
@pytest.mark.parametrize("value", [float("nan"), float("inf"), float("-inf")])
async def test_publish_rejects_nonfinite_json_before_sending(connection, method, value):
    with pytest.raises(ValueError, match="finite"):
        getattr(connection.datachannel.pub_sub, method)(
            "offline/test", {"value": value}
        )


def nested_list(depth):
    value = None
    for _ in range(depth):
        value = [value]
    return value


def test_publish_rejects_excessive_json_depth(connection):
    with pytest.raises(ValueError, match="depth"):
        connection.datachannel.pub_sub.publish_without_callback(
            "offline/test", nested_list(65)
        )


@pytest.mark.parametrize("kind", ["list", "dict"])
def test_publish_rejects_json_cycles(connection, kind):
    value = [] if kind == "list" else {}
    if kind == "list":
        value.append(value)
    else:
        value["self"] = value
    with pytest.raises(ValueError, match="Cyclic"):
        connection.datachannel.pub_sub.publish_without_callback("offline/test", value)


@pytest.mark.parametrize(
    "value",
    [
        nested_list(64),
        {"bool": True, "int": 1, "float": 1.5, "str": "ok", "null": None},
    ],
)
def test_valid_json_reaches_closed_transport(connection, value):
    with pytest.raises(RuntimeError, match="not open"):
        connection.datachannel.pub_sub.publish_without_callback("offline/test", value)


def test_shared_container_is_not_a_cycle(connection):
    shared = [1, 2, 3]
    with pytest.raises(RuntimeError, match="not open"):
        connection.datachannel.pub_sub.publish_without_callback(
            "offline/test", {"left": shared, "right": shared}
        )


@pytest.mark.parametrize("value", [{1: "invalid key"}, {"value": object()}])
def test_publish_rejects_non_json_types(connection, value):
    with pytest.raises(TypeError):
        connection.datachannel.pub_sub.publish_without_callback("offline/test", value)


@pytest.mark.asyncio
@pytest.mark.parametrize("method", ["publish", "publish_request_new"])
@pytest.mark.parametrize("timeout", [-1.0, float("nan"), float("inf"), 1e300])
async def test_invalid_timeout_is_value_error_before_sending(
    connection, method, timeout
):
    with pytest.raises(ValueError, match="timeout"):
        getattr(connection.datachannel.pub_sub, method)(
            "offline/test", {"api_id": 1}, timeout=timeout
        )


@pytest.mark.asyncio
async def test_zero_timeout_is_valid_and_disconnect_is_offline(connection):
    with pytest.raises(RuntimeError, match="not open"):
        await connection.datachannel.pub_sub.publish("offline/test", timeout=0.0)
    await connection.disconnect()
    assert connection.is_connected is False


@pytest.mark.asyncio
async def test_audio_playback_aliases_reject_disconnected_session_without_io(
    connection,
):
    audio = connection.audio
    with pytest.raises(RuntimeError):
        await audio.play_from_file("offline-no-such-file.wav")
    with pytest.raises(RuntimeError):
        await audio.play_from_url("https://example.invalid/offline-no-such-file.wav")
    assert await audio.stop() == ()
