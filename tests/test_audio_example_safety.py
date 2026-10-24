"""Exercise the real audio example runners with offline transport/device fakes."""

import asyncio
import importlib
import sys
import threading
from pathlib import Path
from types import ModuleType, SimpleNamespace
from unittest.mock import AsyncMock

import numpy as np
import pytest

NAMES = ("audio_receive", "audio_play_file", "audio_stream_url")
TX_NAMES = ("audio_play_file", "audio_stream_url")


@pytest.fixture
def audio_examples(monkeypatch, tmp_path):
    events = []
    failures = {}
    hooks = {}
    connections = []
    samples = [np.arange(8, dtype=np.int16)]

    def record(stage):
        events.append(stage)
        if stage in hooks:
            hooks[stage]()
        if stage in failures:
            raise failures[stage]

    class Audio:
        def on_audio(self, callback):
            record("register")
            self.callback = callback

        def switchAudioChannel(self, enabled):
            record("audio_on" if enabled else "audio_off")
            if enabled:
                for frame in samples:
                    self.callback(frame)

        async def play_from_file(self, path):
            record("play_file")

        async def play_from_url(self, url):
            record("play_url")

        async def stop(self):
            record("audio_stop")

    class Connection:
        def __init__(self, method, ip, aes_128_key=None):
            self.ip = ip
            self.key = aes_128_key
            self.audio = Audio()
            self.connect = AsyncMock(side_effect=lambda: record("connect"))
            self.disconnect = AsyncMock(side_effect=lambda: record("disconnect"))
            connections.append(self)

    class Stream:
        def write(self, data):
            record("write")

        def stop_stream(self):
            record("stream_stop")

        def close(self):
            record("stream_close")

    class PyAudio:
        def __init__(self):
            record("pyaudio_init")

        def open(self, **kwargs):
            record("stream_open")
            return Stream()

        def terminate(self):
            record("pyaudio_terminate")

    native = ModuleType("unitree_webrtc_rs")
    native.UnitreeWebRTCConnection = Connection
    native.WebRTCConnectionMethod = SimpleNamespace(LocalSTA=2)
    pyaudio = ModuleType("pyaudio")
    pyaudio.PyAudio = PyAudio
    pyaudio.paInt16 = 8
    monkeypatch.setitem(sys.modules, "unitree_webrtc_rs", native)
    monkeypatch.setitem(sys.modules, "pyaudio", pyaudio)
    monkeypatch.syspath_prepend(str(Path(__file__).resolve().parents[1] / "examples"))
    modules = {}
    for name in ("example_support", *NAMES):
        monkeypatch.delitem(sys.modules, name, raising=False)
        modules[name] = importlib.import_module(name)
    clock = [0.0]

    async def sleep(seconds):
        record("sleep")
        clock[0] += max(seconds, 0.01)

    monkeypatch.setattr(asyncio, "sleep", sleep)
    monkeypatch.setattr(
        modules["audio_receive"],
        "time",
        SimpleNamespace(time=lambda: clock[0], monotonic=lambda: clock[0]),
    )
    # The old receiver uses a module-global wall-clock start; keep the offline
    # reproducer finite without touching a real clock, device or network.
    monkeypatch.setattr(modules["audio_receive"], "test_start", 0.0, raising=False)
    path = tmp_path / "audio.wav"
    path.write_bytes(b"offline placeholder: playback is faked")
    monkeypatch.delenv("ROBOT_IP", raising=False)
    monkeypatch.delenv("UNITREE_AES_128_KEY_FILE", raising=False)
    monkeypatch.delenv("GO2_DORA_UNITREE_AES_128_KEY_FILE", raising=False)
    monkeypatch.delenv("UNITREE_AES_128_KEY", raising=False)
    monkeypatch.delenv("GO2_DORA_UNITREE_AES_128_KEY", raising=False)

    def argv(name, *, target=True, duration=None):
        result = [name]
        if target:
            result.extend(("--ip", "offline.invalid"))
        if name == "audio_play_file":
            result.extend(("--file", str(path)))
        if name == "audio_stream_url":
            result.extend(("--url", "https://user:secret@example.invalid/audio"))
        if duration is not None or name != "audio_receive":
            result.extend(("--duration", duration or "0.05"))
        return result

    def run_main(name, **kwargs):
        monkeypatch.setattr(sys, "argv", argv(name, **kwargs))
        return asyncio.run(modules[name].main())

    return SimpleNamespace(
        modules=modules,
        events=events,
        failures=failures,
        hooks=hooks,
        connections=connections,
        samples=samples,
        argv=argv,
        run_main=run_main,
    )


@pytest.mark.parametrize("name", NAMES)
def test_audio_requires_explicit_robot_target_before_connect(audio_examples, name):
    with pytest.raises(SystemExit) as error:
        audio_examples.run_main(name, target=False)
    assert error.value.code == 2
    assert audio_examples.connections == []


@pytest.mark.parametrize("name", NAMES)
@pytest.mark.parametrize("duration", ("0", "-1", "nan", "inf", "3601"))
def test_invalid_audio_duration_fails_before_connect(audio_examples, name, duration):
    with pytest.raises(SystemExit) as error:
        audio_examples.run_main(name, duration=duration)
    assert error.value.code == 2
    assert audio_examples.connections == []


@pytest.mark.parametrize("name", TX_NAMES)
def test_transmit_start_failure_still_stops_and_disconnects(audio_examples, name):
    stage = "play_file" if name == "audio_play_file" else "play_url"
    audio_examples.failures[stage] = RuntimeError("private credential")
    with pytest.raises(RuntimeError, match="private credential"):
        audio_examples.run_main(name)
    assert audio_examples.events == ["connect", stage, "audio_stop", "disconnect"]


@pytest.mark.parametrize("name", TX_NAMES)
def test_transmit_stop_failure_does_not_skip_disconnect(audio_examples, name, caplog):
    audio_examples.failures["audio_stop"] = RuntimeError("private credential")
    with pytest.raises(RuntimeError):
        audio_examples.run_main(name)
    assert audio_examples.events[-2:] == ["audio_stop", "disconnect"]
    assert "private credential" not in caplog.text


@pytest.mark.parametrize("name", NAMES)
def test_connect_failure_still_disconnects(audio_examples, name):
    audio_examples.failures["connect"] = RuntimeError("private credential")
    with pytest.raises(RuntimeError, match="private credential"):
        audio_examples.run_main(name)
    assert audio_examples.events == ["connect", "disconnect"]


@pytest.mark.parametrize("name", NAMES)
def test_cancellation_propagates_after_audio_and_connection_cleanup(audio_examples, name):
    audio_examples.failures["sleep"] = asyncio.CancelledError()
    with pytest.raises(asyncio.CancelledError):
        audio_examples.run_main(name)
    cleanup = "audio_off" if name == "audio_receive" else "audio_stop"
    assert cleanup in audio_examples.events
    assert audio_examples.events[-1] == "disconnect"


@pytest.mark.parametrize("stage", ("pyaudio_init", "stream_open", "write"))
def test_receive_partial_playback_failure_disables_audio_and_disconnects(audio_examples, stage):
    audio_examples.failures[stage] = RuntimeError("private credential")
    with pytest.raises(RuntimeError, match="private credential"):
        audio_examples.run_main("audio_receive")
    events = audio_examples.events
    assert "audio_off" in events
    assert events[-1] == "disconnect"
    if stage != "pyaudio_init":
        assert "pyaudio_terminate" in events
    if stage == "write":
        assert "stream_stop" in events
        assert "stream_close" in events


@pytest.mark.parametrize("stage", ("audio_off", "stream_stop", "stream_close", "pyaudio_terminate"))
def test_receive_one_cleanup_failure_does_not_skip_other_closers(audio_examples, stage, caplog):
    audio_examples.failures[stage] = RuntimeError("private credential")
    with pytest.raises(RuntimeError):
        audio_examples.run_main("audio_receive")
    for cleanup in (
        "audio_off",
        "stream_stop",
        "stream_close",
        "pyaudio_terminate",
        "disconnect",
    ):
        assert cleanup in audio_examples.events
    assert "private credential" not in caplog.text


def test_receive_drops_newest_overflow_without_blocking_callback(audio_examples, capsys):
    audio_examples.samples[:] = [np.arange(8, dtype=np.int16) for _ in range(6)]
    audio_examples.run_main("audio_receive")
    assert audio_examples.events.count("write") == 4
    assert "dropped=2" in capsys.readouterr().out
    assert audio_examples.events[-1] == "disconnect"


@pytest.mark.parametrize("name", NAMES)
def test_explicit_auth_and_robot_env_reach_connection_without_logging(audio_examples, monkeypatch, name, capsys):
    monkeypatch.setenv("ROBOT_IP", "env-target.invalid")
    monkeypatch.setenv("UNITREE_AES_128_KEY", "private credential")
    audio_examples.run_main(name, target=False)
    assert audio_examples.connections[-1].ip == "env-target.invalid"
    assert audio_examples.connections[-1].key == "private credential"
    output = capsys.readouterr()
    assert "private credential" not in output.out + output.err
    assert "secret@" not in output.out + output.err
    assert "env-target.invalid" not in output.out + output.err


@pytest.mark.parametrize("name", NAMES)
def test_cli_failure_redacts_exception_text(audio_examples, monkeypatch, name, caplog):
    audio_examples.failures["connect"] = RuntimeError("private credential")
    monkeypatch.setattr(sys, "argv", audio_examples.argv(name))
    with pytest.raises(SystemExit) as error:
        audio_examples.modules["example_support"].run(audio_examples.modules[name].main)
    assert error.value.code == 1
    assert "private credential" not in caplog.text


@pytest.mark.parametrize("write_fails", (False, True))
def test_cancelled_playback_write_finishes_before_closing_device(audio_examples, monkeypatch, write_fails, caplog):
    async def scenario():
        loop = asyncio.get_running_loop()
        started = loop.create_future()
        release = threading.Event()

        def blocked_write():
            loop.call_soon_threadsafe(started.set_result, None)
            assert release.wait(timeout=2), "playback write blocked the event loop"
            audio_examples.events.append("write_finished")

        audio_examples.hooks["write"] = blocked_write
        if write_fails:
            audio_examples.failures["write"] = RuntimeError("private credential")
        monkeypatch.setattr(sys, "argv", audio_examples.argv("audio_receive"))
        runner = asyncio.create_task(audio_examples.modules["audio_receive"].main())
        try:
            await asyncio.wait_for(started, timeout=2)
            runner.cancel()
            loop.call_soon(release.set)
            with pytest.raises(asyncio.CancelledError):
                await runner
        finally:
            release.set()
            if not runner.done():
                runner.cancel()
                try:
                    await runner
                except asyncio.CancelledError:
                    pass

    asyncio.run(scenario())
    events = audio_examples.events
    assert events.index("write_finished") < events.index("stream_stop")
    assert events[-1] == "disconnect"
    assert "private credential" not in caplog.text


@pytest.mark.parametrize(
    "samples",
    (
        np.zeros(8, dtype=np.float32),
        np.zeros((2, 4), dtype=np.int16),
        np.zeros(0, dtype=np.int16),
        np.zeros(96_001, dtype=np.int16),
    ),
)
def test_invalid_receive_frame_never_reaches_playback(audio_examples, samples):
    audio_examples.samples[:] = [samples]
    with pytest.raises(RuntimeError, match="No valid audio"):
        audio_examples.run_main("audio_receive")
    assert "write" not in audio_examples.events
    assert "audio_off" in audio_examples.events
    assert audio_examples.events[-1] == "disconnect"


def test_receive_without_optional_pyaudio_still_analyzes_and_cleans_up(audio_examples, monkeypatch):
    monkeypatch.setitem(sys.modules, "pyaudio", None)
    audio_examples.run_main("audio_receive")
    assert "pyaudio_init" not in audio_examples.events
    assert "audio_off" in audio_examples.events
    assert audio_examples.events[-1] == "disconnect"


def test_receive_import_does_not_load_optional_pyaudio(audio_examples, monkeypatch):
    import builtins

    imported = []
    original = builtins.__import__

    def observe(name, *args, **kwargs):
        imported.append(name)
        return original(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", observe)
    monkeypatch.delitem(sys.modules, "audio_receive")
    importlib.import_module("audio_receive")
    assert "pyaudio" not in imported


@pytest.mark.parametrize("stage", ("register", "audio_on"))
def test_receive_setup_failure_still_attempts_channel_off_and_disconnect(audio_examples, stage):
    audio_examples.failures[stage] = RuntimeError("private credential")
    with pytest.raises(RuntimeError, match="private credential"):
        audio_examples.run_main("audio_receive")
    assert audio_examples.events[-2:] == ["audio_off", "disconnect"]


@pytest.mark.parametrize("name", TX_NAMES)
def test_transmit_cleanup_failures_preserve_primary_error(audio_examples, name, caplog):
    stage = "play_file" if name == "audio_play_file" else "play_url"
    audio_examples.failures[stage] = ValueError("primary failure")
    audio_examples.failures["audio_stop"] = RuntimeError("private credential")
    audio_examples.failures["disconnect"] = RuntimeError("private credential")
    with pytest.raises(ValueError, match="primary failure"):
        audio_examples.run_main(name)
    assert audio_examples.events[-2:] == ["audio_stop", "disconnect"]
    assert "private credential" not in caplog.text


def test_receive_cleanup_failures_preserve_primary_error(audio_examples, caplog):
    audio_examples.failures["write"] = ValueError("primary failure")
    audio_examples.failures["audio_off"] = RuntimeError("private credential")
    audio_examples.failures["stream_stop"] = RuntimeError("private credential")
    audio_examples.failures["disconnect"] = RuntimeError("private credential")
    with pytest.raises(ValueError, match="primary failure"):
        audio_examples.run_main("audio_receive")
    assert "stream_close" in audio_examples.events
    assert "pyaudio_terminate" in audio_examples.events
    assert audio_examples.events[-1] == "disconnect"
    assert "private credential" not in caplog.text
