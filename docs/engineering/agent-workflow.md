# SDK agent workflow

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
| Shared bundle selection and host setup | [project routing](../../.agents/README.md), [plugin setup](../../.agents/plugin-setup.md) |
| Task-scoped procedure selection and SDK briefs | This document |

## Task-scoped shared guidance

Generic procedures and their resources have one owner in the shared plugins. The identities below select a bundle and skill, not a local file or a universal host invocation syntax. Read the actual selected entrypoint exposed by the current host; do not hard-code plugin-cache paths or substitute an unrelated same-name copy.

| Guidance identity | Use when |
| --- | --- |
| `rust:rust-best-practices` | Rust implementation or review involving ownership, borrowing, API/error/type design, performance, tests or unsafe/FFI boundaries |
| `common:architecture-patterns` | Responsibility or dependency boundaries change |
| `common:codebase-design` | A scoped module/interface design or refactor needs deeper behavior, clearer responsibility or testable seams; compare alternatives only when useful |
| `common:deprecation-and-migration` | An explicit removal, deprecation decision or API migration needs consumer accounting, compatible aliases and rollback; not routine cleanup or inactivity-based deletion |
| `rust:rust-async-patterns` | Tokio tasks, channels, cancellation or resource lifetime change |
| `common:code-review-excellence` | A source-backed prioritized review is requested |
| `common:docs-sync`, `common:writing-for-agents` | Documentation audit/update or substantive agent-guidance/routing changes, respectively; not an application execution requirement |
| `robotics:robotics-software` | Robot/device I/O, units/frames/clocks, freshness, command/safety authority or reconnect boundaries change; not physical access permission |

The selected plugins are `common@konglabs-agent-skills`, `rust@konglabs-agent-skills` and `robotics@konglabs-agent-skills`; the optional advisory hook is off. The Python bundle is not selected. Python bindings still follow the native SDK contracts below; their presence does not authorize generic Python restructuring or moving Rust hot paths into Python.

### Required selection and evidence

For implementation or substantive review, the following is required:

1. Classify the requested scope and Rust/Python seam, then read the applicable selected-plugin entrypoints and local conventions before editing or reaching a review conclusion. Read detailed chapters only where relevant. A prose-only correction does not load Rust implementation skills or run Cargo checks.
2. Name selected/read procedures and their concrete constraints in one compact update. At completion, connect those rules to actual decisions, observed checks and omissions; a list of names is not evidence of application.
3. Delegate procedure identities, actual source paths, owned files, preserved contracts and allowed checks. Children read those procedures and SDK policy before work. For independent review, verify application against the actual changes and available read/tool evidence, not only the author's declaration.
4. If a procedure is unavailable, disclose the gap and continue clear authorized work using available project guidance; do not claim full coverage, silently install tools, copy prompts or add approval interviews to satisfy a generic example.

This is an evidence/review obligation, not a hard execution hook or a guarantee of semantic compliance. This repository has its own Git root: a parent BattleBang session may not expose this SDK's plugin selection. For an explicitly targeted task, read this AGENTS, routing, conventions and resource policy, and inspect actual host availability. Neither valid metadata nor activation flags prove installation, fresh-session discovery, role registration, model adherence or permission isolation. Unrelated sibling checkouts are not updated by changes here.

### Rust task routing and use cases

Apply the [Rust adaptation rules](conventions.md#9-rust-specific-decisions-for-coding-agents) and [external guide boundaries](conventions.md#10-external-rust-guides-and-project-precedence) with the selected procedure. Architecture/DDD and local-substitution examples do not prescribe this crate's folders, dependencies or port removal; async spawned pool-return, cancel-plus-sleep and lock slogans are not production templates. Selected Rust and Common procedures replace the retired optional expert prompts; none declares this SDK's compiler/MSRV or authorizes nightly features, allocators or frameworks.

| Actual request | Select and inspect | Acceptance within the allowed scope |
| --- | --- | --- |
| Implement or review a Rust API, borrowing or unsafe boundary | `rust:rust-best-practices`, relevant chapters and conventions sections 6/9/10; relevant Google guide headings for style/API judgments | Ownership, error/panic/safety contract and current public consumers; no blanket clone/Arc prohibition, ABI rename or crate rewrite |
| Change a port or extract an independent policy | `common:architecture-patterns`; architecture map, `protocol/ports.rs`, actual consumers | Dependency direction, lifetime/dispatch compatibility, fake-adapter behavior; no speculative service/trait hierarchy |
| Design a deeper module or compare an interface extraction | `common:codebase-design`; relevant deepening/design-comparison resource only; conventions section 11 and actual callers | Preserve aliases/wire fields and explicit lifecycle/FFI contracts; justify seams without adapter quotas, mandatory agent counts or deleting meaningful internal tests |
| Explicitly retire an alias or migrate an API | `common:deprecation-and-migration`; exports, `.pyi`, helpers, examples, tests and known downstream consumers; conventions section 11 | Account for unknown external consumers, compatibility window and rollback; no inactivity-based removal, automatic flags/frameworks, production traffic or hardware execution |
| Fix timeout, reconnect or worker shutdown | `rust:rust-async-patterns` and relevant `rust:rust-best-practices` chapters; `connection/{service,lifecycle,session}.rs`, owning RTC/worker code and resource policy | Cancellation/completion owner, old-epoch isolation, overflow/fault outcome; no sleep-based success, motion replay or Google-platform async ban |
| Change a Python callback or Rust buffer handoff | `rust:rust-best-practices`; async procedure only if concurrency changes; conventions section 6, owning bridge/callback and root `.pyi` | Actual native dtype/shape/ownership/lifetime and callback failure behavior; no mock-only or free-threading claim |
| Review a Rust change | `common:code-review-excellence`; affected code, callers and focused tests | Severity, file:line evidence, smallest causal repair and verification gap; formatting alone is not a blocker |
| Correct prose, links or guidance metadata only | Relevant document/metadata; `common:docs-sync` or `common:writing-for-agents` for substantive scope | Static local-reference/delivery-policy and instruction-conflict checks only; no automatic architecture/async load, application build or robot run |

A bounded implementation brief can name `rust:rust-async-patterns` and ask:

```text
Fix the reported stale-epoch delivery in the owning Rust module. Read the
architecture map, Rust adaptation rules and resource policy; trace producers,
reset and consumers first. Preserve wire fields and Python aliases. Make the
smallest causal repair with a meaningful offline regression. Use applicable
Makefile checks only within scope; no robot connections, dependency upgrades or
unrelated abstractions. Report observed checks separately from proposed checks.
```

## Harness principles

- Keep [AGENTS](../../AGENTS.md) a compact map and disclose detailed context only when relevant. Project rules remain local; generic procedure/resource bodies remain shared.
- Select shared procedures by the task-scoped contract above; do not keep local expert prompts, source manifests, blank form libraries or copied role definitions as a parallel delivery path.
- Express acceptance as relevant observable checks. Guidance-only work uses static checks, not application, service, CI or hardware execution.
- When a failure repeats, improve the smallest shared mechanism or meaningful regression, not a second manual or speculative abstraction. Generic dependencies, coverage, line-count and framework examples grant no extra scope.
- Separate metadata/discovery checks from model adherence and task success. A bounded scenario can supply evidence of a decision; echoed headings or one explicit-invocation rehearsal do not prove automatic selection or universal compliance.

### Responsibilities and available agents

These responsibilities do not require project-specific role files. Claude's Common plugin supplies generic `architect`, `executor`, `code-reviewer` and `verifier` agents when actually loaded. In Codex use built-in or otherwise available host agents with the selected Common procedures and SDK task brief. **Codex plugins do not auto-load custom-agent TOMLs.** Do not recreate project role prompts, copy optional exports, add cache symlinks or claim a role was available without observing it.

| Responsibility | Owns | SDK handoff requirements |
| --- | --- | --- |
| Architecture | Read-only ownership, boundaries and alternatives | Inspect transport/session/task/buffer/Python owners, callers and tests; apply conventions 6/9/10/11, existing ports and the smallest sound seam; distinguish inference from observed behavior |
| Implementation | Bounded change and regression design | Own assigned files, preserve others' work, ABI/wire/feature/resource contracts; use only authorized checks and report concrete procedure-to-decision evidence |
| Independent review | Source-backed regression/complexity findings, not authoring fixes or self-approval | Trace public aliases, bounds, epochs, cancellation/completion and unsafe/FFI; give severity, file:line, failure condition, evidence and smallest repair, not size-rule findings |
| Verification | Claim-to-observed-evidence checks, not implementation edits | Use the actual SDK's feature-aware/native gates only for application scope; parent checks, builds/mocks and graph/navigation output do not prove native correctness, absence of consumers or physical success |

Four responsibilities are not a concurrency limit or a requirement to run four agents. A small prose correction uses scoped static verification; lifecycle/FFI work can justify separate design, implementation, review and verification. The leader retains integration and hardware authority; model/effort follow the session and actual host capability. Preserve task scope; no recursive delegation, publication, hardware, tool installation or widened execution merely because an agent/procedure describes it. If a preferred role is unavailable, disclose that and use an available host agent or direct work with the same bounded responsibility, not a copied local prompt or global-model change.

## Bounded task brief

Copy only relevant fields; do not create a document just to fill a template. Preserve the existing PR/issue forms over shared optional assets.

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

For a read-only lifecycle review, select `rust:rust-async-patterns` and `common:code-review-excellence`:

```text
Read AGENTS.md and relevant conventions/resource policy. Review disconnect/
reconnect ownership in current source and tests. Give prioritized findings with
file:line evidence and the smallest repair/regression needed. No file edits,
builds, test execution, network access or robot connections.
```

For implementation, name the observed failure, permitted files/checks and preserved public contracts. Before robot support expansion, compare sourced model/firmware signaling, auth, topics, codecs and capabilities against current Go2 contracts; unknown capabilities must not fall back to Go2 commands.

## Verification and updates

Run `python3 scripts/check-guidance.py` with an available Python for guidance-only work; `make check-guidance` uses the existing locked environment otherwise. Do not bootstrap application environments for prose. The checker requires AGENTS/README/CONTRIBUTING, checks local Markdown targets in project docs/routing/GitHub guidance, and rejects copied skill entrypoints and project role TOMLs. It has no local source-manifest/hash dependency and uses only Python 3.10-compatible standard-library APIs. It does not validate link anchors, remote provenance, configuration semantics, role registration, installed payloads or model behavior. Parse changed JSON/TOML separately, review instruction conflicts and run `git diff --check`.

For substantive guidance changes, evaluate a bounded use case against actual source and tool evidence, with independent review when warranted and a neighboring prose-only negative case. Evaluate procedure selection, source-backed decisions, preserved contracts and execution scope—not declarations of compliance. Keep observations in the task/PR, not another permanent dated report. Never run application tests/builds, deployments, production traffic or hardware merely to validate guidance.

### Provenance and update ownership

The shared adoption baseline is released `v0.1.1`: provider commit `d56c0780a2d9ce5456d8594fd19d6a19f8bbbb7c`. [Common provenance](https://github.com/KongPedia/agent-skills/blob/d56c0780a2d9ce5456d8594fd19d6a19f8bbbb7c/plugins/common/provenance.json) and [Rust provenance](https://github.com/KongPedia/agent-skills/blob/d56c0780a2d9ce5456d8594fd19d6a19f8bbbb7c/plugins/rust/provenance.json) retain imported source identities, immutable revisions and distinct upstream/source/payload hashes. Their [Common notices](https://github.com/KongPedia/agent-skills/blob/d56c0780a2d9ce5456d8594fd19d6a19f8bbbb7c/plugins/common/THIRD-PARTY-NOTICES.md) and [Rust notices](https://github.com/KongPedia/agent-skills/blob/d56c0780a2d9ce5456d8594fd19d6a19f8bbbb7c/plugins/rust/THIRD-PARTY-NOTICES.md) retain imported licenses/attribution, including the selected wshobson, Apollo, Google, Matt Pocock and Addy Osmani material. Shared procedures, resources, forms, source manifests and notices stay with that provider, not as redundant SDK copies.

SDK-specific compatibility, FFI/lifecycle/resource policy and verification constraints remain in [AGENTS](../../AGENTS.md), [conventions](conventions.md) and [resource policy](resource-policy.md). These policies are not permission to copy upstream excerpts or example code here. The SDK remains Proprietary; retiring guidance copies does not settle application/native-dependency obligations, authorize redistribution or constitute legal clearance.

Shared procedure/resource changes belong to the provider. Template lineage is retained by native Copier in `.copier-answers.yml`; it does not refresh host-managed plugins. Follow [plugin setup](../../.agents/plugin-setup.md) for separate source registration, installation, refresh, project/hook trust and fresh-session checks. The reviewed release baseline, a source channel/ref and configuration flags do not establish currently installed bytes or runtime loading; inspect actual host evidence before claiming either. Do not edit host caches, credentials, global settings or trust to repair guidance, or infer plugin installation, dependency, Git publication or hardware authority from these references.
