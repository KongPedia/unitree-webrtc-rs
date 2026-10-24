"""Release metadata gates use actual manifest/lock/wheel fixtures and CLI statuses."""

import subprocess
import sys
import zipfile
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/check-release.py"


@pytest.fixture
def release(tmp_path):
    (tmp_path / "Cargo.toml").write_text(
        '[package]\nname = "unitree-webrtc-rs"\nversion = "0.3.1"\n[dependencies.other]\nversion = "9.0.0"\n'
    )
    (tmp_path / "pyproject.toml").write_text('[project]\nname = "unitree-webrtc-rs"\nversion = "0.3.1"\n')
    (tmp_path / "Cargo.lock").write_text(
        'version = 4\n[[package]]\nname = "other"\nversion = "9.0.0"\n'
        '[[package]]\nname = "unitree-webrtc-rs"\nversion = "0.3.1"\n'
    )
    (tmp_path / "uv.lock").write_text(
        'version = 1\n[[package]]\nname = "unitree-webrtc-rs"\nversion = "0.3.1"\nsource = { editable = "." }\n'
    )
    return tmp_path


def wheel(root, version="0.3.1", name="unitree-webrtc-rs", *, metadata_count=1):
    path = root / "unitree_webrtc_rs-0.3.1-py3-none-any.whl"
    with zipfile.ZipFile(path, "w") as archive:
        for index in range(metadata_count):
            archive.writestr(
                f"unitree_webrtc_rs_{index}-0.3.1.dist-info/METADATA",
                f"Metadata-Version: 2.3\nName: {name}\nVersion: {version}\n\n",
            )
    return path


def run_check(root, *arguments):
    return subprocess.run(
        [sys.executable, str(SCRIPT), "--root", str(root), *map(str, arguments)],
        capture_output=True,
        text=True,
        check=False,
    )


def test_release_accepts_matching_tag_manifests_locks_and_wheel(release):
    artifact = wheel(release, name="unitree_webrtc_rs")
    result = run_check(release, "--tag", "v0.3.1", "--wheel", artifact)
    assert result.returncode == 0, result.stderr
    assert "0.3.1" in result.stdout
    import hashlib

    assert hashlib.sha256(artifact.read_bytes()).hexdigest() in result.stdout


def test_manual_release_checks_versions_without_requiring_a_tag(release):
    result = run_check(release)
    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize("filename", ["Cargo.toml", "pyproject.toml", "Cargo.lock", "uv.lock"])
def test_release_rejects_any_project_version_mismatch(release, filename):
    path = release / filename
    path.write_text(path.read_text().replace('"0.3.1"', '"0.3.2"'))
    result = run_check(release, "--tag", "v0.3.1")
    assert result.returncode == 1
    assert "version" in result.stderr.lower()
    assert not result.stdout


@pytest.mark.parametrize("tag", ["v0.3.2", "0.3.1", "main", "", "v0.3.1\n"])
def test_release_rejects_tag_not_exactly_matching_project_version(release, tag):
    result = run_check(release, "--tag", tag)
    assert result.returncode == 1
    assert "tag" in result.stderr.lower()


@pytest.mark.parametrize("version,name", [("0.3.2", "unitree-webrtc-rs"), ("0.3.1", "other-project")])
def test_release_rejects_wrong_wheel_metadata_even_with_matching_filename(release, version, name):
    result = run_check(release, "--wheel", wheel(release, version, name))
    assert result.returncode == 1
    assert "wheel" in result.stderr.lower()


@pytest.mark.parametrize("count", [0, 2])
def test_release_rejects_missing_or_ambiguous_wheel_metadata(release, count):
    result = run_check(release, "--wheel", wheel(release, metadata_count=count))
    assert result.returncode == 1
    assert "metadata" in result.stderr.lower()


@pytest.mark.parametrize("filename", ["Cargo.lock", "uv.lock"])
def test_release_rejects_duplicate_local_project_lock_entries(release, filename):
    path = release / filename
    path.write_text(path.read_text() + "\n[[package]]" + path.read_text().split("[[package]]")[-1])
    result = run_check(release)
    assert result.returncode == 1
    assert filename in result.stderr


@pytest.mark.parametrize("filename", ["Cargo.toml", "pyproject.toml", "Cargo.lock", "uv.lock"])
def test_release_reports_missing_manifest_or_lock_as_validation_error(release, filename):
    (release / filename).unlink()
    result = run_check(release)
    assert result.returncode == 1
    assert filename in result.stderr
    assert "Traceback" not in result.stderr


def test_release_reports_corrupt_wheel_as_validation_error(release):
    artifact = wheel(release)
    artifact.write_text("not a zip archive")
    result = run_check(release, "--wheel", artifact)
    assert result.returncode == 1
    assert "wheel" in result.stderr.lower()
    assert "Traceback" not in result.stderr


@pytest.mark.parametrize("filename", ["Cargo.lock", "uv.lock"])
def test_release_rejects_lock_without_local_project_entry(release, filename):
    path = release / filename
    path.write_text(path.read_text().replace('name = "unitree-webrtc-rs"', 'name = "another-project"'))
    result = run_check(release)
    assert result.returncode == 1
    assert filename in result.stderr


@pytest.mark.parametrize("filename", ["Cargo.toml", "pyproject.toml", "Cargo.lock", "uv.lock"])
def test_release_rejects_invalid_toml_without_traceback(release, filename):
    (release / filename).write_text("invalid = [")
    result = run_check(release)
    assert result.returncode == 1
    assert filename in result.stderr
    assert "Traceback" not in result.stderr


def test_release_does_not_print_partial_success_when_later_wheel_fails(release):
    valid = wheel(release)
    other = release / "second.whl"
    other.write_bytes(valid.read_bytes())
    invalid = wheel(release, "0.3.2")
    result = run_check(release, "--wheel", other, "--wheel", invalid)
    assert result.returncode == 1
    assert not result.stdout


def test_release_workflow_preflight_commands_reject_tag_and_version_drift(release):
    import os
    import shutil

    import yaml

    (release / "scripts").mkdir()
    shutil.copyfile(SCRIPT, release / "scripts/check-release.py")
    workflow = yaml.load(
        (ROOT / ".github/workflows/publish-pypi.yml").read_text(),
        Loader=yaml.BaseLoader,
    )
    steps = workflow["jobs"]["release-metadata"]["steps"]
    tag = next(step for step in steps if step.get("env", {}).get("RELEASE_TAG"))
    manual = next(step for step in steps if "workflow_dispatch" in step.get("if", ""))
    env = {
        **os.environ,
        "PATH": f"{Path(sys.executable).parent}:{os.environ['PATH']}",
        "RELEASE_TAG": "v9.9.9",
    }
    tagged = subprocess.run(
        ["bash", "-e", "-c", tag["run"]],
        cwd=release,
        env=env,
        capture_output=True,
        text=True,
    )
    assert tagged.returncode == 1
    assert "tag" in tagged.stderr
    dispatched = subprocess.run(
        ["bash", "-e", "-c", manual["run"]],
        cwd=release,
        env=env,
        capture_output=True,
        text=True,
    )
    assert dispatched.returncode == 0, dispatched.stderr
    (release / "pyproject.toml").write_text('[project]\nname = "unitree-webrtc-rs"\nversion = "0.3.2"\n')
    drifted = subprocess.run(
        ["bash", "-e", "-c", manual["run"]],
        cwd=release,
        env=env,
        capture_output=True,
        text=True,
    )
    assert drifted.returncode == 1
    assert "version" in drifted.stderr
