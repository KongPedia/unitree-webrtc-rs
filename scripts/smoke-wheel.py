#!/usr/bin/env python3
"""Fresh-install a wheel and run offline native contracts outside the checkout."""

import os
import shutil
import subprocess
import sys
import tempfile
from importlib.metadata import version
from pathlib import Path


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: smoke-wheel.py path/to/package.whl")
    wheel = Path(sys.argv[1]).resolve(strict=True)
    if wheel.suffix != ".whl":
        raise SystemExit("expected a wheel file")
    env = {
        key: value
        for key, value in os.environ.items()
        if key
        not in {
            "PYTHONPATH",
            "PYTHONHOME",
            "UNITREE_WEBRTC_RS_LOG",
            "UNITREE_WEBRTC_RS_LOG_INIT",
            "PYTEST_ADDOPTS",
            "PYTEST_PLUGINS",
            "PYTEST_DISABLE_PLUGIN_AUTOLOAD",
        }
    }
    with tempfile.TemporaryDirectory(prefix="unitree-wheel-") as directory:
        work = Path(directory)
        subprocess.run(
            ["uv", "venv", "--python", sys.executable, str(work / "venv")],
            cwd=work,
            env=env,
            check=True,
        )
        python = work / "venv" / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
        subprocess.run(
            [
                "uv",
                "pip",
                "install",
                "--python",
                str(python),
                str(wheel),
                f"pytest=={version('pytest')}",
                f"pytest-asyncio=={version('pytest-asyncio')}",
            ],
            cwd=work,
            env=env,
            check=True,
        )
        subprocess.run(
            [
                str(python),
                "-I",
                "-c",
                """
import importlib
from importlib.machinery import ExtensionFileLoader
from importlib.metadata import distribution
from pathlib import Path
import sys
import unitree_webrtc_rs as sdk
binary = importlib.import_module('unitree_webrtc_rs.unitree_webrtc_rs')
assert isinstance(binary.__loader__, ExtensionFileLoader)
assert Path(binary.__file__).resolve().is_relative_to(Path(sys.prefix).resolve())
assert sdk.UnitreeWebRTCConnection is binary.UnitreeWebRTCConnection
files = distribution('unitree-webrtc-rs').files
assert any(str(path).endswith('.pyi') for path in files), 'wheel missing stubs'
connection = sdk.UnitreeWebRTCConnection(sdk.WebRTCConnectionMethod.LocalSTA, ip='127.0.0.1')
assert connection.is_connected is connection.isConnected is False
assert connection.connection_method == connection.connectionMethod
assert connection.datachannel.ready_state == connection.datachannel.readyState
print('clean wheel: native loader, shipped stub and legacy aliases OK; no robot connection')
print('installed extension:', binary.__file__)
print('Python:', sys.version)
for package in ('unitree-webrtc-rs', 'numpy', 'pytest', 'pytest-asyncio'):
    print(package + ':', distribution(package).version)
""",
            ],
            cwd=work,
            env=env,
            check=True,
        )

        # Tests are checkout resources, not assumed wheel contents; copy only native/offline contracts.
        tests = work / "tests"
        tests.mkdir()
        source = Path(__file__).resolve().parents[1] / "tests"
        for filename in (
            "test_native_contracts.py",
            "test_native_logging.py",
            "test_native_input_contracts.py",
            "test_native_callback_lifetime.py",
        ):
            shutil.copyfile(source / filename, tests / filename)
        config = work / "pytest.ini"
        config.write_text("[pytest]\nasyncio_mode = strict\nasyncio_default_fixture_loop_scope = function\n")
        subprocess.run(
            [
                str(python),
                "-I",
                "-m",
                "pytest",
                "-q",
                "--import-mode=importlib",
                "-c",
                str(config),
                str(tests),
            ],
            cwd=work,
            env=env,
            check=True,
        )


if __name__ == "__main__":
    main()
