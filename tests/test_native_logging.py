"""Process-isolated logging contracts against the rebuilt native extension."""

import os
import subprocess
import sys

import pytest


def run_import(log_filter=None, source="import unitree_webrtc_rs", *, log_init=None):
    env = os.environ.copy()
    env.pop("UNITREE_WEBRTC_RS_LOG", None)
    env.pop("UNITREE_WEBRTC_RS_LOG_INIT", None)
    if log_init is not None:
        env["UNITREE_WEBRTC_RS_LOG_INIT"] = log_init
    if log_filter is not None:
        env["UNITREE_WEBRTC_RS_LOG"] = log_filter
    return subprocess.run(
        [sys.executable, "-c", source], env=env, capture_output=True, text=True
    )


@pytest.mark.parametrize("log_filter", [None, "warn", "unitree_webrtc_rs=info"])
def test_normal_import_is_quiet(log_filter):
    result = run_import(log_filter)
    assert result.returncode == 0, result.stderr
    assert result.stdout == ""
    assert result.stderr == ""


def test_debug_import_uses_stderr_and_target_once():
    result = run_import(
        "unitree_webrtc_rs=debug",
        "import importlib; import unitree_webrtc_rs; "
        "importlib.reload(unitree_webrtc_rs); import unitree_webrtc_rs",
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == ""
    assert result.stderr.count("module_initialized") == 1
    assert "unitree_webrtc_rs" in result.stderr
    assert "DEBUG" in result.stderr


def test_invalid_explicit_filter_is_value_error_without_echoing_input():
    result = run_import(
        "unitree_webrtc_rs=secret-canary-invalid-level",
        "try:\n import unitree_webrtc_rs\n"
        "except ValueError as error:\n print(type(error).__name__)\n"
        "else:\n raise AssertionError('invalid filter accepted')",
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == "ValueError\n"
    assert "secret-canary" not in result.stderr


@pytest.mark.parametrize(
    "log_filter", ["unitree_webrtc_rs=debug", "secret-invalid-level"]
)
def test_disabled_auto_init_does_not_install_or_parse_filter(log_filter):
    result = run_import(log_filter, log_init="0")
    assert result.returncode == 0, result.stderr
    assert result.stdout == ""
    assert result.stderr == ""


def test_invalid_auto_init_setting_is_value_error_without_echoing_input():
    result = run_import(
        source="try:\n import unitree_webrtc_rs\n"
        "except ValueError as error:\n print(type(error).__name__)\n"
        "else:\n raise AssertionError('invalid init setting accepted')",
        log_init="secret-invalid-init",
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == "ValueError\n"
    assert result.stderr == ""


def test_explicit_auto_init_enabled_preserves_sdk_filter():
    result = run_import("unitree_webrtc_rs=debug", log_init="1")
    assert result.returncode == 0, result.stderr
    assert result.stdout == ""
    assert "module_initialized" in result.stderr
