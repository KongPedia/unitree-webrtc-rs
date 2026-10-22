# Local agent workflow

## Source-of-truth map

Keep each rule in its owning document; entry points link to details instead of repeating them.

| Question | Canonical location |
| --- | --- |
| Task scope, authorization and review priorities | [AGENTS.md](../../AGENTS.md) |
| Current composition, dataflow and practical extension seams | [architecture map](../architecture_analysis.md) |
| Responsibilities, naming, compatibility, logging and verification policy | [conventions.md](conventions.md) |
| Implemented lifecycle, queue/buffer bounds and Rust migration | [resource-policy.md](resource-policy.md) |
| Installation, public API, example safety and validation limits | [README.md](../../README.md) |
| Contributor setup/check selection | [CONTRIBUTING.md](../../CONTRIBUTING.md) |
| Release matrix, authentication and publication gates | [publishing guide](../../.github/PUBLISHING.md) |
| Local skill selection and task briefs | This document |

## Task-scoped upstream guidance

| Guidance | Use when |
| --- | --- |
| [architecture-patterns](../../.agents/skills/architecture-patterns/SKILL.md) | Responsibility or dependency boundaries change |
| [rust-async-patterns](../../.agents/skills/rust-async-patterns/SKILL.md) | Tokio tasks, channels, cancellation or resource lifetime change |
| [code-review-excellence](../../.agents/skills/code-review-excellence/SKILL.md) | A source-backed prioritized review is requested |
| [rust-pro.md](upstream-prompts/rust-pro.md) | Optional ownership/type-system/FFI reference |
| [refactor-clean.md](upstream-prompts/refactor-clean.md) | Optional conditional/SRP cleanup reference |

These are pinned copies from [wshobson/agents](https://github.com/wshobson/agents); revision and SHA-256 values are in [skill-sources.json](skill-sources.json), with the upstream [MIT notice](upstream-license.txt). Keep vendored skills, references and prompts upstream-identical. Project policy belongs in AGENTS/conventions, not in upstream bodies. Generic dashboard, dependency, coverage and line-count examples do not authorize extra scope.

Use only guidance relevant to the task. Invoke a skill by `$skill-name`; if another installed copy has the same name, identify the exact local `SKILL.md` path instead of loading both. The two prompt files are references, not registered Codex commands/roles; their `model: opus` metadata does not select the current Codex model. No global/plugin-cache changes are required.

### Rust task routing and use cases

Apply the [Rust adaptation rules](conventions.md#9-rust-specific-decisions-for-coding-agents) with the selected skill. The architecture skill's Python/DDD examples do not prescribe this crate's folders; the async skill's spawned pool-return, cancel-plus-sleep and lock slogans are not production templates. `rust-pro` is a broad reference, not a compiler/MSRV declaration or authorization for nightly features, allocators or new frameworks.

| Actual request | Select and inspect | Acceptance to propose within the allowed scope |
| --- | --- | --- |
| Change a port or extract an independent policy | `architecture-patterns`; current architecture map, `protocol/ports.rs`, actual consumers | Dependency direction, lifetime/dispatch compatibility, fake-adapter behavior; no speculative service/trait hierarchy |
| Fix timeout, reconnect or worker shutdown | `rust-async-patterns`; `connection/{service,lifecycle,session}.rs`, owning RTC/worker code and resource policy | Cancellation/completion owner, old-epoch isolation, overflow/fault outcome; no sleep-based success or motion replay |
| Change a Python callback or buffer handoff | `rust-async-patterns` only if concurrency changes; conventions section 6, owning bridge/callback and root `.pyi` | Actual native dtype/shape/ownership/lifetime and callback failure behavior; no mock-only or free-threading claim |
| Review a Rust change | `code-review-excellence`; affected code, callers and focused tests | Severity, file:line evidence, smallest causal repair and verification gap; formatting alone is not a blocker |
| Correct prose, links or skill metadata only | Relevant document/metadata; no automatic architecture/async skill load | Guidance metadata/hash/reference and instruction-conflict checks only; no application build or robot run |

For example, a bounded Rust implementation request can use the existing task brief:

```text
$rust-async-patterns
Fix the reported stale-epoch delivery in the owning Rust module. Read the
architecture map, Rust adaptation rules and resource policy; trace producers,
reset and consumers first. Preserve wire fields and Python aliases. Propose the
smallest causal repair, then implement with a meaningful offline regression.
Use applicable Makefile checks; no robot connections, dependency upgrades or
unrelated abstractions. Report observed checks separately from proposed checks.
```

## Harness principles

- Keep AGENTS a compact map and disclose detailed context only when relevant, following [OpenAI's harness-engineering account](https://openai.com/index/harness-engineering/).
- Give each rule one canonical home; check the actual source/configuration before promoting a note into a rule.
- Apply the three skills by task scope; the two expert prompts remain optional references, not mandatory ceremony.
- Express acceptance as relevant executable checks; document-only work uses metadata/reference/diff checks, not application or hardware execution.
- When a failure repeats, inspect the common mechanism and improve the smallest shared rule or regression check. [OpenAI's workflow example](https://developers.openai.com/cookbook/examples/codex/iterating-development-workflows-with-codex) is a convention to adapt, not a requirement to create more files.
- Separate file metadata/discovery checks from model adherence and task success. [Codex skill documentation](https://learn.chatgpt.com/docs/build-skills) describes invocation/discovery; hashes and valid metadata do not prove that an agent selects or follows a skill correctly.

## Bounded task brief

Copy only fields relevant to the task. Do not create a planning/review document just to fill a template.

```text
Outcome and evidence: <observed problem, source references or sanitized trace>
Scope / ownership: <files/modules; integration owner>
Allowed execution / exclusions: <offline checks; hardware/external limits>
Compatibility: <imports, aliases, kwargs, wire fields, array dtype/shape>
Resource contract: <owner, queue/overflow, cancellation and shutdown>
Smallest complete change: <preserved behavior; rejected extra scope>
Acceptance: <focused regression; native/packaging checks when relevant>
Report: <changed files, exact results, evidence versus inference, remaining gaps>
```

For example, a read-only lifecycle review:

```text
$rust-async-patterns $code-review-excellence
Read AGENTS.md and the relevant conventions/resource policy. Review disconnect/
reconnect ownership in the current source and tests. Give prioritized findings
with file:line evidence and the smallest repair/regression needed. No file edits,
builds, test execution, network access or robot connections.
```

For an implementation task, name the observed failure, permitted files/checks and preserved public contracts. Add `architecture-patterns` only when responsibility boundaries change. Before adding robot support, compare sourced model/firmware signaling, auth, topics, codecs and capabilities against current Go2 contracts; unknown capabilities must not fall back to Go2 commands.

## Verification and updates

Run `make check-guidance` for metadata, pinned hashes and local Markdown references. Inspect the diff and instruction conflicts separately. This validates files, not live UI discovery, automatic skill selection or model compliance. A fresh session may be needed to observe changed instruction/skill discovery; verify that in the actual host rather than claiming it from hashes.

For a substantive guidance change, try a bounded task from the use-case table with an independent agent and raw source context, without supplying the expected answer. Include a neighboring negative case such as a prose-only correction. Evaluate selected guidance, source-backed decisions, preserved contracts and execution scope—not echoed headings or claimed compliance. Keep observations in the task/PR, not another permanent dated report. [OpenAI's skill-evaluation example](https://developers.openai.com/blog/eval-skills) motivates this distinction; one explicit-invocation rehearsal does not prove automatic discovery or universal adherence.

For a deliberate upstream upgrade, compare a reviewed pinned revision in a temporary destination, replace only the owned source/reference/prompt copies, refresh manifest hashes/license and rerun guidance checks. Do not track `main` automatically or edit marketplace caches. Keep application tests/builds and hardware checks out of prose-only verification.
