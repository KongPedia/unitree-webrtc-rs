"""Offline regression coverage for repository guidance and build ordering."""

import importlib.util
import json
from pathlib import Path
import subprocess

import pytest

ROOT = Path(__file__).resolve().parents[1]


def validator():
    spec = importlib.util.spec_from_file_location(
        "check_guidance", ROOT / "scripts/check-guidance.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def guidance(tmp_path):
    (tmp_path / ".agents/skills/demo").mkdir(parents=True)
    (tmp_path / "docs/engineering").mkdir(parents=True)
    skill = tmp_path / ".agents/skills/demo/SKILL.md"
    skill.write_text("---\nname: demo\ndescription: Example guidance.\n---\n")
    import hashlib

    manifest = {
        "files": [
            {
                "local": str(skill.relative_to(tmp_path)),
                "sha256": hashlib.sha256(skill.read_bytes()).hexdigest(),
            }
        ]
    }
    (tmp_path / "docs/engineering/skill-sources.json").write_text(json.dumps(manifest))
    (tmp_path / "AGENTS.md").write_text(
        "[skill](.agents/skills/demo/SKILL.md#usage) [web](https://example.org) [anchor](#here)\n"
    )
    (tmp_path / "README.md").write_text("# SDK\n")
    (tmp_path / "CONTRIBUTING.md").write_text("# Contributing\n")
    return tmp_path, skill


def test_guidance_accepts_local_fragment_and_external_links(guidance):
    root, _ = guidance
    assert validator().check(root) == []


def test_guidance_detects_modified_pinned_copy(guidance):
    root, skill = guidance
    skill.write_text(skill.read_text() + "changed\n")
    assert any("SHA-256" in error for error in validator().check(root))


@pytest.mark.parametrize(
    "metadata",
    [
        "name: [demo]",
        "name: demo\nname: duplicate\ndescription: text",
        "name: demo\ndescription: |\n  text",
        "name demo\ndescription: text",
    ],
)
def test_metadata_rejects_malformed_or_unsupported_yaml(metadata):
    with pytest.raises(ValueError):
        validator().skill_metadata(f"---\n{metadata}\n---\n")


def test_guidance_detects_missing_local_link(guidance):
    root, _ = guidance
    (root / "AGENTS.md").write_text("[missing](docs/missing.md#section)")
    assert any("missing local link" in error for error in validator().check(root))


@pytest.mark.parametrize(
    "document", ["README.md", ".github/PUBLISHING.md", "docs/nested/context.md"]
)
def test_guidance_checks_all_maintained_documents(guidance, document):
    root, _ = guidance
    path = root / document
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("[obsolete](missing-plan.md)")
    assert any("missing local link" in error for error in validator().check(root))


@pytest.mark.parametrize("document", ["AGENTS.md", "README.md", "CONTRIBUTING.md"])
def test_guidance_requires_entry_documents(guidance, document):
    root, _ = guidance
    (root / document).unlink()
    assert any(
        "missing required document" in error for error in validator().check(root)
    )


@pytest.mark.parametrize("change", ["empty", "duplicate", "unlisted", "outside"])
def test_guidance_requires_complete_unique_manifest(guidance, change):
    root, _ = guidance
    path = root / "docs/engineering/skill-sources.json"
    manifest = json.loads(path.read_text())
    if change == "empty":
        manifest["files"] = []
    elif change == "duplicate":
        manifest["files"] *= 2
    elif change == "unlisted":
        (root / ".agents/skills/demo/context.md").write_text("pinned context")
    else:
        manifest["files"][0]["local"] = "../outside.md"
    path.write_text(json.dumps(manifest))
    assert validator().check(root)


def test_release_installs_and_probes_runtime_plugins_before_acceptance():
    workflow = (ROOT / ".github/workflows/publish-pypi.yml").read_text()
    for package in (
        "gstreamer1.0-tools",
        "gstreamer1.0-plugins-base",
        "gstreamer1.0-plugins-good",
        "gstreamer1.0-plugins-bad",
        "gstreamer1.0-libav",
    ):
        assert package in workflow
    assert workflow.index("gst-inspect-1.0 --exists") < workflow.index(
        "make acceptance"
    )


def test_native_tests_build_first_even_parallel_dry_run():
    result = subprocess.run(
        ["make", "-n", "-j8", "test-python"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    )
    assert result.stdout.index("maturin develop") < result.stdout.index("pytest -q")


def test_check_preserves_full_feature_lint_and_default_feature_check():
    result = subprocess.run(
        ["make", "-n", "check"], cwd=ROOT, capture_output=True, text=True, check=True
    )
    assert (
        "cargo clippy --locked --all-targets --all-features -- -D warnings"
        in result.stdout
    )
    assert "cargo check --locked --tests\n" in result.stdout


def test_release_gates_precede_artifact_upload():
    workflow = (ROOT / ".github/workflows/publish-pypi.yml").read_text()
    assert workflow.index("make acceptance") < workflow.index("maturin build")
    assert workflow.index("scripts/smoke-wheel.py") < workflow.index(
        "actions/upload-artifact"
    )
    assert "needs: build-wheels" in workflow


def test_parallel_make_keeps_build_and_tests_serial(tmp_path):
    import os

    fake_uv = tmp_path / "uv"
    log = tmp_path / "commands.log"
    fake_uv.write_text(
        "#!/usr/bin/env python3\n"
        "import os, subprocess, sys, time\n"
        "args = sys.argv[4:]\n"
        "if args[:2] == ['python', '-c']:\n"
        "    raise SystemExit(subprocess.call([sys.executable, *args[1:]]))\n"
        "with open(os.environ['HARNESS_LOG'], 'a') as log:\n"
        "    log.write('start ' + ' '.join(args) + '\\n')\n"
        "time.sleep(0.02)\n"
        "with open(os.environ['HARNESS_LOG'], 'a') as log:\n"
        "    log.write('end ' + ' '.join(args) + '\\n')\n"
    )
    fake_uv.chmod(0o755)
    env = {
        **os.environ,
        "PATH": f"{tmp_path}:{os.environ['PATH']}",
        "HARNESS_LOG": str(log),
    }
    subprocess.run(
        ["make", "-j8", "check", "test-python"],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        check=True,
    )
    lines = log.read_text().splitlines()
    for start, end in zip(lines[::2], lines[1::2]):
        assert start.startswith("start ")
        assert end == "end " + start.removeprefix("start ")
    assert lines.index("end maturin develop --locked") < lines.index("start pytest -q")


def test_wheel_smoke_uses_project_interpreter_via_uv(tmp_path, monkeypatch):
    import sys

    spec = importlib.util.spec_from_file_location(
        "smoke_wheel", ROOT / "scripts/smoke-wheel.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    wheel = tmp_path / "offline.whl"
    wheel.touch()
    monkeypatch.setattr(sys, "argv", [str(ROOT / "scripts/smoke-wheel.py"), str(wheel)])
    monkeypatch.setenv("PYTHONHOME", "must-not-leak")
    monkeypatch.setenv("PYTHONPATH", "must-not-leak")
    monkeypatch.setenv("UV_OFFLINE", "1")
    calls = []
    monkeypatch.setattr(
        module.subprocess, "run", lambda args, **kwargs: calls.append((args, kwargs))
    )
    module.main()
    assert len(calls) == 3
    create, install, smoke = calls
    assert create[0] == [
        "uv",
        "venv",
        "--python",
        sys.executable,
        str(create[1]["cwd"] / "venv"),
    ]
    assert install[0][:4] == ["uv", "pip", "install", "--python"]
    assert install[0][-1] == str(wheel.resolve())
    assert smoke[0][1:3] == ["-I", "-c"]
    assert "ExtensionFileLoader" in smoke[0][3]
    for _, kwargs in calls:
        assert not kwargs["cwd"].is_relative_to(ROOT)
        assert kwargs["check"] is True
        assert kwargs["env"]["UV_OFFLINE"] == "1"
        assert "PYTHONHOME" not in kwargs["env"]
        assert "PYTHONPATH" not in kwargs["env"]
