"""Version tooling uses fake lock commands; no builds, publishing or dependency fetches."""

import json
import os
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "sync-version.sh"
FILES = ("Cargo.toml", "pyproject.toml", "Cargo.lock", "uv.lock")


@pytest.fixture
def repository(tmp_path):
    (tmp_path / "Cargo.toml").write_text(
        '[package]\nname = "unitree-webrtc-rs"\nversion = "0.2.0"\n'
        '\n[dependencies.example]\nversion = "8.0.0"\n'
    )
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname = "unitree-webrtc-rs"\nversion = "0.2.0"\n'
        '\n[tool.example]\nversion = "9.0.0"\n'
    )
    for name in ("Cargo.lock", "uv.lock"):
        (tmp_path / name).write_text(f"original {name}\n")
    commands = tmp_path / "bin"
    commands.mkdir()
    for name in ("uv", "cargo"):
        executable = commands / name
        executable.write_text(
            f"#!{sys.executable}\n"
            "import json, os, pathlib, re, sys\n"
            "name = pathlib.Path(sys.argv[0]).name\n"
            "with open('commands.jsonl', 'a') as log:\n"
            "    log.write(json.dumps([name, *sys.argv[1:]]) + '\\n')\n"
            "expected = ['lock'] if name == 'uv' else ['update', '--workspace']\n"
            "if sys.argv[1:] != expected: sys.exit(23)\n"
            "manifest, lock = ('pyproject.toml', 'uv.lock') if name == 'uv' "
            "else ('Cargo.toml', 'Cargo.lock')\n"
            'version = re.search(r\'version = "([^"]+)"\', '
            "pathlib.Path(manifest).read_text()).group(1)\n"
            "pathlib.Path(lock).write_text('locked ' + version + '\\n')\n"
            "sys.exit(17 if os.environ.get('FAIL_COMMAND') == name else 0)\n"
        )
        executable.chmod(0o755)
    return tmp_path


def run_sync(repository, *arguments, fail=None):
    env = {**os.environ, "PATH": f"{repository / 'bin'}:{os.environ['PATH']}"}
    if fail:
        env["FAIL_COMMAND"] = fail
    return subprocess.run(
        ["bash", str(SCRIPT), *arguments],
        cwd=repository,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )


def snapshot(repository):
    return {name: (repository / name).read_bytes() for name in FILES}


def test_updates_only_project_versions_and_refreshes_both_locks(repository):
    before = snapshot(repository)
    result = run_sync(repository, "0.3.0")
    assert result.returncode == 0, result.stderr
    for name in ("Cargo.toml", "pyproject.toml"):
        assert (repository / name).read_bytes() == before[name].replace(
            b'"0.2.0"', b'"0.3.0"'
        )
    assert (repository / "Cargo.lock").read_text() == "locked 0.3.0\n"
    assert (repository / "uv.lock").read_text() == "locked 0.3.0\n"
    calls = [
        json.loads(line)
        for line in (repository / "commands.jsonl").read_text().splitlines()
    ]
    assert calls == [
        ["uv", "lock"],
        ["cargo", "update", "--workspace"],
    ]
    assert "git add ." not in result.stdout


@pytest.mark.parametrize(
    "arguments",
    [
        (),
        ("",),
        ("1.2",),
        ("1.2.3\n9.9.9",),
        ("1.2.3; touch unwanted",),
        ("1.2.3", "unexpected"),
        ("01.2.3",),
    ],
)
def test_invalid_version_never_edits_files(repository, arguments):
    before = snapshot(repository)
    result = run_sync(repository, *arguments)
    assert result.returncode != 0
    assert snapshot(repository) == before
    assert not (repository / "commands.jsonl").exists()


@pytest.mark.parametrize("fail", ["uv", "cargo"])
def test_failed_lock_refresh_rolls_back_all_four_files(repository, fail):
    before = snapshot(repository)
    result = run_sync(repository, "0.3.0", fail=fail)
    assert result.returncode == 17
    assert snapshot(repository) == before
    assert "synchronized" not in result.stdout


@pytest.mark.parametrize("manifest", ["Cargo.toml", "pyproject.toml"])
def test_missing_project_version_rolls_back_without_lock_commands(repository, manifest):
    path = repository / manifest
    path.write_text(path.read_text().replace('version = "0.2.0"\n', ""))
    before = snapshot(repository)
    result = run_sync(repository, "0.3.0")
    assert result.returncode != 0
    assert snapshot(repository) == before
    assert not (repository / "commands.jsonl").exists()


def test_actual_cargo_refreshes_root_package_version_offline(repository):
    cargo = shutil.which("cargo")
    if cargo is None:
        pytest.skip("Cargo is unavailable")
    (repository / "Cargo.toml").write_text(
        '[package]\nname = "unitree-webrtc-rs"\nversion = "0.2.0"\nedition = "2021"\n'
    )
    (repository / "src").mkdir()
    (repository / "src/lib.rs").touch()
    subprocess.run(
        [cargo, "generate-lockfile", "--offline"],
        cwd=repository,
        check=True,
        capture_output=True,
        text=True,
    )
    # Actual Cargo, offline; uv remains fake to avoid dependency resolution/network.
    (repository / "bin/cargo").write_text(
        f'#!/bin/bash\nexec {shlex.quote(cargo)} "$@" --offline\n'
    )
    result = run_sync(repository, "0.3.0")
    assert result.returncode == 0, result.stderr
    assert (
        'name = "unitree-webrtc-rs"\nversion = "0.3.0"'
        in (repository / "Cargo.lock").read_text()
    )
