"""Offline callback-reference lifetime contracts against the real extension."""

import gc
import importlib
import subprocess
import sys
import weakref
from importlib.machinery import ExtensionFileLoader

import pytest


@pytest.fixture(scope="module")
def native():
    native = importlib.import_module("unitree_webrtc_rs")
    binary = importlib.import_module("unitree_webrtc_rs.unitree_webrtc_rs")
    assert isinstance(binary.__loader__, ExtensionFileLoader)
    assert native.UnitreeWebRTCConnection is binary.UnitreeWebRTCConnection
    return native


@pytest.fixture
def connection(native):
    return native.UnitreeWebRTCConnection(native.WebRTCConnectionMethod.LocalSTA, ip="127.0.0.1")


@pytest.mark.asyncio
@pytest.mark.parametrize("media,method", [("video", "on_frame"), ("audio", "on_audio")])
async def test_disconnect_retains_registered_callback(connection, media, method):
    class Callback:
        def __call__(self, _frame):
            pass

    register = getattr(getattr(connection, media), method)
    callback = Callback()
    retained = weakref.ref(callback)
    register(callback)
    del callback

    assert await connection.disconnect() == ()
    gc.collect()
    assert retained() is not None

    # Replacing with an independent callable also releases the old registration.
    register(lambda _frame: None)
    assert retained() is None


@pytest.mark.asyncio
@pytest.mark.parametrize("media,method", [("video", "on_frame"), ("audio", "on_audio")])
@pytest.mark.parametrize("kind", ["bound", "closure"])
async def test_clear_callbacks_releases_captured_connection_owner(native, media, method, kind):
    connection = native.UnitreeWebRTCConnection(native.WebRTCConnectionMethod.LocalSTA, ip="127.0.0.1")

    class Owner:
        def __init__(self, root):
            self.connection = root

        def callback(self, _frame):
            pass

    owner = Owner(connection)
    released = weakref.ref(owner)
    callback = owner.callback if kind == "bound" else lambda _frame, owner=owner: None
    getattr(getattr(connection, media), method)(callback)
    del callback, owner
    gc.collect()
    assert released() is not None

    assert connection.clear_callbacks() is None
    assert await connection.disconnect() == ()
    del connection
    gc.collect()
    assert released() is None


def test_clear_callbacks_allows_later_registration(connection):
    class Callback:
        def __call__(self, _frame):
            pass

    assert connection.clear_callbacks() is None
    assert connection.clear_callbacks() is None
    callback = Callback()
    released = weakref.ref(callback)
    connection.video.on_frame(callback)
    del callback
    assert released() is not None

    connection.clear_callbacks()
    assert released() is None
    assert connection.is_connected is False


@pytest.mark.parametrize("media,method", [("video", "on_frame"), ("audio", "on_audio")])
def test_clear_callbacks_reentrant_finalizer_can_register_again(media, method):
    code = """
import gc
import sys
import weakref

from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod

connection = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="127.0.0.1")
register = getattr(getattr(connection, sys.argv[1]), sys.argv[2])
events = []
replacement_refs = []

class Replacement:
    def __call__(self, _frame):
        pass

class Callback:
    def __call__(self, _frame):
        pass

    def __del__(self):
        replacement = Replacement()
        replacement_refs.append(weakref.ref(replacement))
        register(replacement)
        events.append("finalized")

register(Callback())
assert connection.clear_callbacks() is None
assert events == ["finalized"], events
assert replacement_refs[0]() is not None
connection.clear_callbacks()
gc.collect()
assert replacement_refs[0]() is None
"""
    subprocess.run(
        [sys.executable, "-c", code, media, method],
        check=True,
        timeout=10,
        capture_output=True,
        text=True,
    )


@pytest.mark.asyncio
async def test_unit_future_results_remain_empty_tuples(connection):
    assert await connection.audio.stop() == ()
    assert await connection.disconnect() == ()
