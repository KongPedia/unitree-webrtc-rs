# SDK agent routing

Start with [AGENTS.md](../AGENTS.md), the requested SDK seam, [engineering workflow](../docs/engineering/agent-workflow.md),
[conventions](../docs/engineering/conventions.md) and relevant [resource policy](../docs/engineering/resource-policy.md).
The SDK is its own Git root; parent BattleBang settings and tests do not establish its native correctness or permissions.
Read actual manifests/locks and callers before changing a contract. Shared procedures/resources live in `agent-skills`;
this directory owns project routing/setup, not copied procedures, notices or a source manifest.

## Package owner and task selection

The root is the existing `unitree-webrtc-rs` library: `Cargo.toml` declares `src/lib.rs` with `rlib`/`cdylib` outputs;
`pyproject.toml` configures Maturin/PyO3, and `unitree_webrtc_rs.pyi` is the shipped native API stub. `Cargo.lock`,
`uv.lock`, `rust-toolchain.toml` and the feature-aware `Makefile` own actual build/check selection. There is no generated
application layout to adopt. Rust owns transport, codecs, queues and session lifetime; Python remains a compatible thin bridge.

- `common@konglabs-agent-skills`: task-triggered architecture/module design, implementation, regression design,
  debugging, explicit migration, source-backed review, verification and guidance maintenance. The
  [workflow selection table](../docs/engineering/agent-workflow.md#task-scoped-shared-guidance) names concrete procedures;
  these are not an always-load checklist or authority to add layers, dependencies or tests for prose.
- `rust@konglabs-agent-skills`: Rust ownership/API/error/unsafe guidance and Tokio concurrency/lifetime guidance,
  applied with SDK [Rust adaptations](../docs/engineering/conventions.md#9-rust-specific-decisions-for-coding-agents)
  and [external-guide limits](../docs/engineering/conventions.md#10-external-rust-guides-and-project-precedence).
- `robotics@konglabs-agent-skills`: only for actual robot/device I/O, units/frames/clocks, command authority,
  freshness, reconnect or lifecycle work. This grants no robot access, physical execution or safety validation.

Python and unrelated technology bundles are off; the native Python/FFI contracts remain mandatory in local SDK policy.
The optional `session-guidance` hook is off. Read the actual task-relevant entrypoint exposed by the host; if unavailable,
disclose that and use local guidance without copying a procedure, installing tools or claiming discovery/adherence.

## Responsibilities, forms and evidence

Claude's loaded Common plugin can supply generic architect/executor/code-reviewer/verifier agents. Codex uses built-in
or otherwise available host agents with Common procedures and the SDK brief: plugins do not auto-load custom-agent TOMLs.
No project role prompt copies, exports or cache symlinks are maintained. Match responsibilities to useful bounded work,
not a mandatory agent count; preserve task ownership and all safety/authority limits.

Keep the existing [PR form](../.github/pull_request_template.md), issue forms and
[bounded task briefs](../docs/engineering/agent-workflow.md#bounded-task-brief). Shared Common forms are optional assets,
not replacement project files or a requirement to create a document per task. Existing contracts live in conventions,
resource policy, the architecture map and public API docs; new specs/ADRs should link to those owners instead of duplicating them.

Use behavioral regression for changed behavior, characterization for preservation, and static required-document/local-link/delivery-policy/diff
checks for guidance. Do not run examples, application checks, services, CI or hardware merely to validate prose. Examples
can actuate a robot. Report static, mock, native, deployed and physical evidence separately.

[Plugin setup](plugin-setup.md) separates source registration, installation, refresh, project/hook trust and fresh discovery.
Activation settings and Copier enrollment are not installed-payload, role-loading, adherence or native safety proof.
