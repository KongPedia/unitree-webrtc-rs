<!-- AUTONOMY DIRECTIVE — DO NOT REMOVE -->
YOU ARE AN AUTONOMOUS CODING AGENT. EXECUTE TASKS TO COMPLETION WITHOUT ASKING FOR PERMISSION.
DO NOT STOP TO ASK "SHOULD I PROCEED?" — PROCEED. DO NOT WAIT FOR CONFIRMATION ON OBVIOUS NEXT STEPS.
IF BLOCKED, TRY AN ALTERNATIVE APPROACH. ONLY ASK WHEN TRULY AMBIGUOUS OR DESTRUCTIVE.
USE CODEX NATIVE SUBAGENTS FOR INDEPENDENT PARALLEL SUBTASKS WHEN THAT IMPROVES THROUGHPUT. THIS IS COMPLEMENTARY TO OMX TEAM MODE.
<!-- END AUTONOMY DIRECTIVE -->

# unitree-webrtc-rs working agreement

## Scope and entry points

- Follow the latest user scope and inherited working agreement. Skills, roles and keyword routing do not expand authorization. Classify work as review, planning, application implementation, or documentation/harness maintenance.
- Read `git status --short` first; preserve unrelated edits. No commit, push, publish, issue posting, deployment or robot connection/control unless authorized for that task. Examples can issue physical commands; they are not offline tests.
- Complete clear reversible work without repeated permission handoffs. Parallelize genuinely independent bounded work when useful; give each agent ownership, evidence requirements and a no-recursive-orchestration boundary. Keep shared contracts and file edits under one integration owner.
- Read [engineering conventions](docs/engineering/conventions.md) before application changes. Use [agent workflow and task templates](docs/engineering/agent-workflow.md) for planning/review.
- Use the existing `wshobson/agents` skills in `.agents/skills/`: `$architecture-patterns` for responsibility/dependency boundaries, `$rust-async-patterns` for Tokio/task/channel lifetime, and `$code-review-excellence` for source-backed reviews. Apply the [Rust adaptation rules](docs/engineering/conventions.md#9-rust-specific-decisions-for-coding-agents) before copying generic examples; [agent-workflow.md](docs/engineering/agent-workflow.md) maps skills to concrete Rust tasks. No custom replacement skill is required.

## Repository boundaries

- Rust owns transport, codecs, queues and session lifetime. Python bindings marshal inputs/outputs and expose async operations; do not move hot-path processing back into Python.
- Reuse `src/protocol/ports.rs` and existing adapters. Keep protocol contracts free of PyO3/GStreamer/HTTP implementation dependencies; bind concrete adapters at the composition root.
- Search all callers, exports, stubs, examples and tests before changing a public interface. Preserve existing camelCase aliases, wire field names, topic strings and command IDs; use idiomatic names for new internal code.
- Distinguish protocol constants from tunable policy and per-device secrets. Do not scatter robot/firmware branches, turn every constant into configuration, or add speculative profiles/plugins for unverified robots.
- Another Unitree model is supported only when its transport, auth, topics, codecs and capability differences have evidence. Upstream Python support is not Rust-port or hardware proof. Unsupported capabilities must not silently send Go2 commands.

## Correctness and performance

- Each worker/task/channel needs an owner, bounded resource policy and explicit shutdown path. Do not treat `Arc` as lifecycle management, a bounded queue as guaranteed delivery, or a session ID as complete cancellation.
- Validate lengths, sizes, ranges, timeouts and schemes at external boundaries before allocation, slicing, GStreamer pipeline construction or sending. Malformed peer/user input must return an error, not panic or silently truncate.
- Python callbacks execute attached to Python. Keep Rust compute and blocking waits detached where appropriate. Never hold callback-registry locks while invoking user code; surface callback errors without exposing secrets.
- Describe zero-copy by boundary: Rust-owned buffer to NumPy, not network-to-Python unless the whole path is measured. Document dtype, shape, layout, ownership and lifetime. Every `unsafe` block/impl needs a safety rationale; do not infer CPython free-threading safety from ordinary GIL-path testing.
- Prefer the smallest causal repair and regression test over fallback layering, unnecessary traits or mass renaming. Before refactoring, write a short cleanup plan stating responsibilities, affected callers and preserved behavior.
- Prefer shallow control flow: guard clauses, `?`, `Option`/`Result`, exhaustive enum `match`, and readable iterators. Consolidate repeated model/policy decisions at one boundary; use strategy only for real varying behavior. Do not ban necessary codec/event loops or replace `if` with an abstraction that merely hides the same complexity.
- Upstream skills are guidance, not application scaffolding requirements: do not copy example dependencies, create DDD repositories/microservices, force arbitrary function/class size limits or add dashboards solely because a generic prompt illustrates them.

## Verification and reporting

- Application changes: inspect `Makefile`, run the smallest relevant regression first, then applicable `make check` and Rust/Python checks. `make test-rust` uses the locked Python environment and Python-linked Rust tests; extension builds alone enable `extension-module`. Report failures rather than weakening checks. Mock Python tests do not validate the native extension; add/use native offline contract coverage when changing bindings.
- Documentation/skill-only changes: validate applicable guidance metadata/references, inspect diffs and review instruction conflicts. An ordinary prose correction does not require loading unrelated skills. Do not run application tests, builds, CI, services or hardware merely to verify prose.
- Report changed files, exact checks/results, and unverified limits. Separate static evidence, software tests, deployed behavior and physical validation. New performance claims require reproducible workload/platform/version evidence.
- Existing commit subjects use `[BTB-<id>] <type>. <summary>`; do not invent a ticket. When authorized to commit, explain intent, constraints and verification gaps in the body.

## Code Review Rules

- Prioritize input/resource bounds, session teardown/reconnect, reliable request completion, callback failures and compatibility over cosmetic pattern compliance.
- Flag new unbounded queues/reassembly maps, silent send failures, Go2 fallback for unknown devices, and examples labeled read-only that issue state-changing commands.
- Confirm findings against current source and tests; reviews and historical observations are not live implementation truth.
