"""Offline regression coverage for repository guidance and build ordering."""

import importlib.util
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


def validator():
    spec = importlib.util.spec_from_file_location("check_guidance", ROOT / "scripts/check-guidance.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def guidance(tmp_path):
    (tmp_path / ".agents").mkdir()
    (tmp_path / ".agents/README.md").write_text("# Project routing\n")
    (tmp_path / "AGENTS.md").write_text(
        "[routing](.agents/README.md#usage) [web](https://example.org) [anchor](#here)\n"
    )
    (tmp_path / "README.md").write_text("# SDK\n")
    (tmp_path / "CONTRIBUTING.md").write_text("# Contributing\n")
    return tmp_path


def test_guidance_accepts_plugin_only_local_fragment_and_external_links(guidance):
    assert validator().check(guidance) == []


def test_guidance_detects_missing_local_link(guidance):
    (guidance / "AGENTS.md").write_text("[missing](docs/missing.md#section)")
    assert any("missing local link" in error for error in validator().check(guidance))


@pytest.mark.parametrize(
    "document", ["README.md", ".agents/README.md", ".github/PUBLISHING.md", "docs/nested/context.md"]
)
def test_guidance_checks_all_maintained_documents(guidance, document):
    path = guidance / document
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("[obsolete](missing-plan.md)")
    assert any("missing local link" in error for error in validator().check(guidance))


@pytest.mark.parametrize("document", ["AGENTS.md", "README.md", "CONTRIBUTING.md"])
def test_guidance_requires_entry_documents(guidance, document):
    (guidance / document).unlink()
    assert any("missing required document" in error for error in validator().check(guidance))


@pytest.mark.parametrize("copied", [".agents/skills/demo/SKILL.md", ".codex/agents/executor.toml"])
def test_guidance_rejects_copied_shared_skills_and_project_roles(guidance, copied):
    path = guidance / copied
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("copied guidance")
    assert any("copied guidance" in error for error in validator().check(guidance))


def test_release_installs_and_probes_runtime_plugins_before_acceptance():
    import shlex

    steps = release_build()["steps"]
    packages = {
        "gstreamer1.0-tools",
        "gstreamer1.0-plugins-base",
        "gstreamer1.0-plugins-good",
        "gstreamer1.0-plugins-bad",
        "gstreamer1.0-libav",
    }
    install = next(index for index, step in enumerate(steps) if packages <= set(shlex.split(step.get("run", ""))))
    probe = next(index for index, step in enumerate(steps) if "gst-inspect-1.0" in shlex.split(step.get("run", "")))
    acceptance = next(
        index for index, step in enumerate(steps) if shlex.split(step.get("run", "")) == ["make", "acceptance"]
    )
    assert install < probe < acceptance


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
    result = subprocess.run(["make", "-n", "check"], cwd=ROOT, capture_output=True, text=True, check=True)
    assert "cargo clippy --locked --all-targets --all-features -- -D warnings" in result.stdout
    assert "cargo check --locked --tests\n" in result.stdout


def test_release_gates_precede_artifact_upload():
    import shlex

    steps = release_build()["steps"]
    acceptance = next(
        index for index, step in enumerate(steps) if shlex.split(step.get("run", "")) == ["make", "acceptance"]
    )
    build = next(index for index, step in enumerate(steps) if "maturin" in shlex.split(step.get("run", "")))
    wheel_gate = next(
        index
        for index, step in enumerate(steps)
        if "scripts/check-release.py" in shlex.split(step.get("run", ""))
        and "--wheel" in shlex.split(step.get("run", ""))
    )
    smoke = next(
        index for index, step in enumerate(steps) if "scripts/smoke-wheel.py" in shlex.split(step.get("run", ""))
    )
    upload = next(
        index for index, step in enumerate(steps) if step.get("uses", "").startswith("actions/upload-artifact@")
    )
    assert acceptance < build < wheel_gate < smoke < upload
    for index in (build, wheel_gate, smoke, upload):
        assert steps[index]["if"] == "inputs.build-wheel == true"
    caller = workflows()["publish-pypi.yml"]["jobs"]["build-wheels"]
    assert caller["with"]["build-wheel"] == "true"


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
    for start, end in zip(lines[::2], lines[1::2], strict=True):
        assert start.startswith("start ")
        assert end == "end " + start.removeprefix("start ")
    assert lines.index("end maturin develop --locked") < lines.index("start pytest -q -c pyproject.toml")


def test_wheel_smoke_uses_project_interpreter_via_uv(tmp_path, monkeypatch):
    import sys

    spec = importlib.util.spec_from_file_location("smoke_wheel", ROOT / "scripts/smoke-wheel.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    wheel = tmp_path / "offline.whl"
    wheel.touch()
    monkeypatch.setattr(sys, "argv", [str(ROOT / "scripts/smoke-wheel.py"), str(wheel)])
    monkeypatch.setenv("PYTHONHOME", "must-not-leak")
    monkeypatch.setenv("PYTHONPATH", "must-not-leak")
    monkeypatch.setenv("UV_OFFLINE", "1")
    calls = []
    monkeypatch.setattr(module.subprocess, "run", lambda args, **kwargs: calls.append((args, kwargs)))
    module.main()
    assert len(calls) == 4
    create, install, smoke, native_tests = calls
    assert create[0] == [
        "uv",
        "venv",
        "--python",
        sys.executable,
        str(create[1]["cwd"] / "venv"),
    ]
    assert install[0][:4] == ["uv", "pip", "install", "--python"]
    assert install[0][5] == str(wheel.resolve())
    assert any(argument.startswith("pytest==") for argument in install[0])
    assert any(argument.startswith("pytest-asyncio==") for argument in install[0])
    assert native_tests[0][1:4] == ["-I", "-m", "pytest"]
    assert smoke[0][1:3] == ["-I", "-c"]
    assert "ExtensionFileLoader" in smoke[0][3]
    for _, kwargs in calls:
        assert not kwargs["cwd"].is_relative_to(ROOT)
        assert kwargs["check"] is True
        assert kwargs["env"]["UV_OFFLINE"] == "1"
        assert "PYTHONHOME" not in kwargs["env"]
        assert "PYTHONPATH" not in kwargs["env"]


def test_make_invalidates_pyo3_config_when_python_changes_at_same_path(tmp_path):
    import os

    fake_uv = tmp_path / "uv"
    fake_uv.write_text(
        "#!/usr/bin/env python3\n"
        "import os, sys, sysconfig\n"
        "sys.version = os.environ['HARNESS_PYTHON_VERSION']\n"
        "sysconfig.get_config_var = lambda name: os.environ['HARNESS_PYTHON_ABI']\n"
        "exec(sys.argv[-1])\n"
    )
    fake_uv.chmod(0o755)
    probe = tmp_path / "probe.mk"
    probe.write_text('print-python-signature:;@printf "%s\\n" "$$PYO3_ENVIRONMENT_SIGNATURE"\n')
    signatures = []
    for version, abi in (("3.10.19", "cpython-310"), ("3.12.15", "cpython-312")):
        result = subprocess.run(
            [
                "make",
                "--no-print-directory",
                "-f",
                "Makefile",
                "-f",
                str(probe),
                "print-python-signature",
            ],
            cwd=ROOT,
            env={
                **os.environ,
                "PATH": f"{tmp_path}:{os.environ['PATH']}",
                "HARNESS_PYTHON_VERSION": version,
                "HARNESS_PYTHON_ABI": abi,
            },
            capture_output=True,
            text=True,
            check=True,
        )
        signatures.append(result.stdout.strip())
    assert all(signatures)
    assert signatures[0] != signatures[1]


def test_release_target_cache_isolated_by_python_arch_and_rust():
    import re

    steps = release_build()["steps"]
    target_cache = next(step["with"] for step in steps if step.get("with", {}).get("path") == "target")
    context = {
        "runner.os": "Linux",
        "runner.arch": "X64",
        "steps.build-cache.outputs.python": "3.10.20-cpython-310",
        "steps.build-cache.outputs.rust": "rust-1.92.0",
        "hashFiles('**/Cargo.lock')": "lock-a",
    }

    def render(template, values):
        return re.sub(r"\$\{\{ (.*?) \}\}", lambda match: values[match[1]], template)

    original = render(target_cache["key"], context)
    for field in context:
        assert render(target_cache["key"], {**context, field: "different"}) != original
        if not field.startswith("hashFiles"):
            assert render(target_cache["restore-keys"], {**context, field: "different"}) != render(
                target_cache["restore-keys"], context
            )
    identity = next(index for index, step in enumerate(steps) if step.get("id") == "build-cache")
    sync = next(index for index, step in enumerate(steps) if "uv sync" in step.get("run", ""))
    assert sync < identity


def workflows():
    import yaml

    return {
        path.name: yaml.load(path.read_text(), Loader=yaml.BaseLoader)
        for path in (ROOT / ".github/workflows").glob("*.yml")
    }


def release_build():
    configs = workflows()
    job = configs["publish-pypi.yml"]["jobs"]["build-wheels"]
    if "uses" in job:
        return configs[Path(job["uses"]).name]["jobs"]["acceptance"]
    return job


@pytest.mark.parametrize(
    "branch, has_pull_request, expected_events",
    [
        ("main", False, ["push"]),
        ("feat/sdk-assurance-hardening", True, ["pull_request"]),
        ("feat/sdk-assurance-hardening", False, []),
        ("fix/offline-checks", True, ["pull_request"]),
    ],
)
def test_acceptance_event_policy_avoids_duplicate_feature_pr_runs(branch, has_pull_request, expected_events):
    import fnmatch

    triggers = workflows()["acceptance.yml"]["on"]
    push = triggers["push"]
    accepted_events = []
    if any(fnmatch.fnmatchcase(branch, pattern) for pattern in push["branches"]):
        accepted_events.append("push")
    if has_pull_request and "pull_request" in triggers:
        accepted_events.append("pull_request")
    assert accepted_events == expected_events
    assert set(push) == {"branches"}, "tag pushes must use the release workflow, not direct acceptance"


def test_pull_requests_and_main_pushes_run_read_only_acceptance_without_publish():
    candidates = [config for config in workflows().values() if {"pull_request", "push"} <= config["on"].keys()]
    assert candidates, "no PR/main acceptance workflow is configured"
    for config in candidates:
        assert not config["on"]["pull_request"], "fork and same-repository PRs must remain unfiltered"
        assert "workflow_call" in config["on"], "release builds must retain reusable acceptance"
        assert config["permissions"] == {"contents": "read"}
        assert config["concurrency"]["cancel-in-progress"] == "true"
        assert "github.ref" in config["concurrency"]["group"]
        for job in config["jobs"].values():
            assert "if" not in job, "acceptance must not skip fork PR jobs"
            assert "environment" not in job
            assert "permissions" not in job
            for step in job.get("steps", []):
                assert not step.get("uses", "").startswith("pypa/gh-action-pypi-publish@")


def test_release_build_requires_version_preflight_and_publish_requires_success():
    config = workflows()["publish-pypi.yml"]
    needs = config["jobs"]["build-wheels"].get("needs", [])
    assert "release-metadata" in needs
    assert config["jobs"]["publish-pypi"]["needs"] == "build-wheels"
    metadata = config["jobs"]["release-metadata"]
    tag_gate = next(step for step in metadata["steps"] if step.get("env", {}).get("RELEASE_TAG"))
    assert "push" in tag_gate["if"]
    assert tag_gate["env"]["RELEASE_TAG"] == "${{ github.ref_name }}"


def test_release_toolchain_is_read_from_repository_manifest(tmp_path):
    import os

    try:
        import tomllib
    except ImportError:
        import tomli as tomllib

    job = release_build()
    step = next((step for step in job["steps"] if step.get("id") == "toolchain"), None)
    assert step is not None, "release must resolve the checked-in Rust toolchain"
    output = tmp_path / "outputs"
    subprocess.run(
        ["bash", "-e", "-c", step["run"]],
        cwd=ROOT,
        env={**os.environ, "GITHUB_OUTPUT": str(output)},
        check=True,
        capture_output=True,
        text=True,
    )
    resolved = dict(line.split("=", 1) for line in output.read_text().splitlines())
    expected = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]
    assert resolved["channel"] == expected["channel"]
    assert set(resolved["components"].split(",")) == set(expected["components"])
    installer = next(step for step in job["steps"] if step.get("uses", "").startswith("dtolnay/rust-toolchain@"))
    assert installer["with"]["toolchain"] == "${{ steps.toolchain.outputs.channel }}"
    assert installer["with"]["components"] == "${{ steps.toolchain.outputs.components }}"


def test_fresh_wheel_runs_native_contract_and_logging_subset_outside_checkout(tmp_path, monkeypatch):
    import sys

    spec = importlib.util.spec_from_file_location("smoke_wheel", ROOT / "scripts/smoke-wheel.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    artifact = tmp_path / "offline.whl"
    artifact.touch()
    monkeypatch.setattr(sys, "argv", [str(ROOT / "scripts/smoke-wheel.py"), str(artifact)])
    observed = []

    def run(args, **kwargs):
        if "pytest" not in args:
            return
        copied = kwargs["cwd"] / "tests"
        assert not copied.is_relative_to(ROOT)
        assert (copied / "test_native_contracts.py").read_bytes() == (
            ROOT / "tests/test_native_contracts.py"
        ).read_bytes()
        assert (copied / "test_native_logging.py").read_bytes() == (ROOT / "tests/test_native_logging.py").read_bytes()
        assert all(path.name.startswith("test_native_") for path in copied.iterdir())
        assert args[1:4] == ["-I", "-m", "pytest"]
        observed.append(args)

    monkeypatch.setattr(module.subprocess, "run", run)
    module.main()
    assert observed, "fresh-installed wheel must execute meaningful native regressions"


def test_external_actions_are_pinned_and_checkout_does_not_persist_credentials():
    import re

    for config in workflows().values():
        for job in config["jobs"].values():
            for step in job.get("steps", []):
                action = step.get("uses")
                if not action or action.startswith("./"):
                    continue
                assert re.fullmatch(r"[^@]+@[0-9a-f]{40}", action), action
                if action.startswith("actions/checkout@"):
                    assert step["with"]["persist-credentials"] == "false"


def test_release_evidence_is_not_downloaded_as_a_publishable_package():
    import fnmatch

    job = workflows()["publish-pypi.yml"]["jobs"]["publish-pypi"]
    download = next(
        step["with"] for step in job["steps"] if step.get("uses", "").startswith("actions/download-artifact@")
    )
    upload_names = [
        step["with"]["name"]
        for step in release_build()["steps"]
        if step.get("uses", "").startswith("actions/upload-artifact@")
    ]
    assert len(upload_names) == 2
    assert fnmatch.fnmatch("wheels-macos-14-3.10", download["pattern"])
    assert not fnmatch.fnmatch("wheel-evidence-macos-14-3.10", download["pattern"])


def test_native_pytest_recipe_ignores_ancestor_repository_configuration(tmp_path):
    import os
    import sys

    # A submodule's locked pytest environment must not require the parent's plugins/options.
    (tmp_path / "pyproject.toml").write_text('[tool.pytest.ini_options]\naddopts = "--parent-plugin-not-installed"\n')
    sdk = tmp_path / "sdk"
    (sdk / "tests").mkdir(parents=True)
    (sdk / "pyproject.toml").write_bytes((ROOT / "pyproject.toml").read_bytes())
    (sdk / "tests/test_isolation.py").write_text("def test_sdk_isolation():\n    assert True\n")
    wrapper = tmp_path / "bin"
    wrapper.mkdir()
    uv = wrapper / "uv"
    uv.write_text(
        f"#!{sys.executable}\nimport os, sys\nos.execv(sys.executable, [sys.executable, '-m', *sys.argv[4:]])\n"
    )
    uv.chmod(0o755)
    dry_run = subprocess.run(["make", "-n", "test-python"], cwd=ROOT, capture_output=True, text=True, check=True)
    recipe = next(line for line in dry_run.stdout.splitlines() if line.startswith("if [ -d tests ]"))
    result = subprocess.run(
        ["bash", "-e", "-c", recipe],
        cwd=sdk,
        env={**os.environ, "PATH": f"{wrapper}:{os.environ['PATH']}"},
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert "1 passed" in result.stdout
