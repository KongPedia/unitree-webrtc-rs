"""Bounded JSON inputs against the real extension, without opening a transport."""

import importlib
import inspect
from importlib.machinery import ExtensionFileLoader

import pytest


@pytest.fixture(scope="module")
def pubsub():
    native = importlib.import_module("unitree_webrtc_rs")
    binary = importlib.import_module("unitree_webrtc_rs.unitree_webrtc_rs")
    assert isinstance(binary.__loader__, ExtensionFileLoader)
    connection = native.UnitreeWebRTCConnection(native.WebRTCConnectionMethod.LocalSTA, ip="127.0.0.1")
    return connection.datachannel.pub_sub


def shared_tree():
    value = None
    for _ in range(15):
        value = [value, value]
    return value


@pytest.mark.asyncio
@pytest.mark.parametrize("method", ["publish", "publish_request_new", "publish_without_callback"])
@pytest.mark.parametrize(
    "value",
    [
        pytest.param([None] * 32_768, id="wide-list"),
        pytest.param({str(i): None for i in range(20_000)}, id="wide-object"),
        pytest.param(shared_tree(), id="expanded-shared-tree"),
        pytest.param("x" * (1024 * 1024 + 1), id="large-string"),
        pytest.param("é" * 600_000, id="large-utf8-string"),
        pytest.param({"x" * (1024 * 1024 + 1): None}, id="large-key"),
        pytest.param(["x" * 65_536] * 17, id="aggregate-strings"),
    ],
)
async def test_oversized_json_is_rejected_before_transport(pubsub, method, value):
    with pytest.raises(ValueError, match="budget"):
        result = getattr(pubsub, method)("offline/test", value)
        if inspect.isawaitable(result):
            await result


@pytest.mark.asyncio
@pytest.mark.parametrize("method", ["publish", "publish_request_new", "publish_without_callback"])
@pytest.mark.parametrize("value", [-9223372036854775809, 18446744073709551616, 10**100])
async def test_out_of_range_integer_is_not_rounded_to_float(pubsub, method, value):
    with pytest.raises(ValueError, match="integer"):
        result = getattr(pubsub, method)("offline/test", {"value": value})
        if inspect.isawaitable(result):
            await result


def test_small_shared_json_remains_valid(pubsub):
    shared = [True, 9223372036854775809, "ok"]
    with pytest.raises(RuntimeError, match="not open"):
        pubsub.publish_without_callback("offline/test", [shared, shared])


def test_moderate_aggregate_strings_reach_transport(pubsub):
    with pytest.raises(RuntimeError, match="not open"):
        pubsub.publish_without_callback("offline/test", ["x" * 32_768] * 8)
