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
| [rust-best-practices](../../.agents/skills/rust-best-practices/SKILL.md) | Rust implementation or review involving ownership, borrowing, API/error/type design, performance, tests or unsafe/FFI boundaries |
| [architecture-patterns](../../.agents/skills/architecture-patterns/SKILL.md) | Responsibility or dependency boundaries change |
| [rust-async-patterns](../../.agents/skills/rust-async-patterns/SKILL.md) | Tokio tasks, channels, cancellation or resource lifetime change |
| [code-review-excellence](../../.agents/skills/code-review-excellence/SKILL.md) | A source-backed prioritized review is requested |
| [rust-pro.md](upstream-prompts/rust-pro.md) | Optional ownership/type-system/FFI reference |
| [refactor-clean.md](upstream-prompts/refactor-clean.md) | Optional conditional/SRP cleanup reference |

The existing three skills and two prompt references come from [wshobson/agents](https://github.com/wshobson/agents), with its [MIT notice](upstream-license.txt). The general Rust skill comes from [Apollo](https://github.com/apollographql/skills); its references include a pinned [Google Rust Style Guide](../../.agents/skills/rust-best-practices/references/google-rust-style-guide.md). Source revisions, licenses, original/local hashes and explicit adaptations are in [skill-sources.json](skill-sources.json). Rows without `source_id` retain the original top-level wshobson repository/revision; new rows resolve their named source instead. Preserve upstream bodies unless an adaptation is recorded. Project policy belongs in AGENTS/conventions. Generic dependencies, coverage and line-count examples do not authorize extra scope.

An ordinary task description is enough: select relevant guidance without requiring `$skill-name` from the user. Explicit invocation is optional. If another installed copy has the same name, read the exact path in this table, not both copies. The two prompt files are references, not registered Codex commands/roles; their `model: opus` metadata does not select the current Codex model. No global/plugin-cache changes are required.

### Required selection and evidence

For implementation or substantive review, the following is required:

1. Classify the requested scope and Rust seam, then read the applicable exact local `SKILL.md` files before editing or reaching a review conclusion. Read detailed chapters only where relevant. A prose-only correction does not load Rust implementation skills or run Cargo checks.
2. Name selected skills and their concrete constraints in one compact update. At completion, connect those rules to actual decisions, observed checks and omissions; a list of skill names is not evidence of application.
3. Delegate exact local paths, owned files, preserved contracts and allowed checks. Children read those paths before work. For changes requiring independent review, verify application against the actual changes and available read/tool evidence, not only the author's declaration.
4. If a selected file is missing, disclose the gap and use available project guidance without claiming full coverage or silently installing tools. Keep clear authorized work moving; do not add repeated approval interviews or new scope to satisfy generic examples.

This is a project instruction and evidence/review obligation, not a hard execution hook or a guarantee of semantic compliance. Skills are selected from descriptions; valid files, hashes and names alone do not prove automatic use. This repository has its own Git root: a parent BattleBang session may not list these nested skills initially. For an explicitly targeted task, read this repository's AGENTS and the exact local paths above. A standalone checkout must contain its own guidance; unrelated sibling checkouts are not updated by changes here.

### Rust task routing and use cases

Apply the [Rust adaptation rules](conventions.md#9-rust-specific-decisions-for-coding-agents) and [external guide boundaries](conventions.md#10-external-rust-guides-and-project-precedence) with the selected skill. The architecture skill's Python/DDD examples do not prescribe this crate's folders; the async skill's spawned pool-return, cancel-plus-sleep and lock slogans are not production templates. `rust-pro` is a broad reference, not a compiler/MSRV declaration or authorization for nightly features, allocators or new frameworks.

| Actual request | Select and inspect | Acceptance to propose within the allowed scope |
| --- | --- | --- |
| Implement or review a Rust API, borrowing or unsafe boundary | `rust-best-practices`, relevant chapters and conventions sections 6/9/10; relevant Google guide headings for style/API judgments | Ownership, error/panic/safety contract and current public consumers; no blanket clone/Arc prohibition, ABI rename or crate rewrite |
| Change a port or extract an independent policy | `architecture-patterns`; current architecture map, `protocol/ports.rs`, actual consumers | Dependency direction, lifetime/dispatch compatibility, fake-adapter behavior; no speculative service/trait hierarchy |
| Fix timeout, reconnect or worker shutdown | `rust-async-patterns` and relevant `rust-best-practices` chapters; `connection/{service,lifecycle,session}.rs`, owning RTC/worker code and resource policy | Cancellation/completion owner, old-epoch isolation, overflow/fault outcome; no sleep-based success, motion replay or Google-platform async ban |
| Change a Python callback or Rust buffer handoff | `rust-best-practices`; `rust-async-patterns` only if concurrency changes; conventions section 6, owning bridge/callback and root `.pyi` | Actual native dtype/shape/ownership/lifetime and callback failure behavior; no mock-only or free-threading claim |
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
- Select local skills by the required task-scoped contract above; the two expert prompts remain optional references, not mandatory ceremony.
- Express acceptance as relevant executable checks; document-only work uses metadata/reference/diff checks, not application or hardware execution.
- When a failure repeats, inspect the common mechanism and improve the smallest shared rule or regression check. [OpenAI's workflow example](https://developers.openai.com/cookbook/examples/codex/iterating-development-workflows-with-codex) is a convention to adapt, not a requirement to create more files.
- Separate file metadata/discovery checks from model adherence and task success. [Codex skill documentation](https://learn.chatgpt.com/docs/build-skills) describes invocation/discovery; hashes and valid metadata do not prove that an agent selects or follows a skill correctly.

### Roles and best-fit use cases

The local `.codex/agents` files provide four model-free responsibilities for this repository, independent of a parent checkout:

| Role | Owns | Suitable use |
| --- | --- | --- |
| [architect](../../.codex/agents/architect.toml) | Read-only ownership, boundaries and alternatives | Protocol/FFI/resource-contract design or broad refactor planning |
| [executor](../../.codex/agents/executor.toml) | Bounded implementation with relevant Rust skills | Feature/fix/refactor, regression design and authorized offline checks |
| [code-reviewer](../../.codex/agents/code-reviewer.toml) | Independent source-backed regression/complexity review | Resource, concurrency, compatibility and safety-sensitive changes |
| [verifier](../../.codex/agents/verifier.toml) | Claim-to-observed-evidence verification | Feature-aware Rust/native Python outcomes or static guidance checks |

Four role files are categories, not a concurrency limit or a requirement to run all four. A small prose correction needs only scoped static verification; a lifecycle/FFI change can use architecture, owned implementation, separate review and verification. Debugging and test design normally combine an existing role with the relevant skill and task brief. Add another role only for a demonstrated distinct tool/context/evidence need, not a job title.

The leader retains integration and hardware authority boundaries. Model/effort inherit from the session; no TOML proves account availability, fresh-session registration or isolation from parent permissions. If a role cannot run, disclose it and use an available inherited-model agent with the bounded role instructions rather than changing global model settings. [Official subagent guidance](https://learn.chatgpt.com/docs/agent-configuration/subagents) describes the registration surface; metadata parsing is not an execution claim.

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

The existing checker validates metadata, local pinned hashes and Markdown references. For guidance-only work with an available Python, run `python scripts/check-guidance.py` directly; `make check-guidance` otherwise uses the locked UV environment. Do not bootstrap application environments merely for prose. Inspect the diff, upstream source identities/original hashes and instruction conflicts separately: the checker does not validate the new multi-source provenance semantics. This validates files, not live UI discovery, automatic selection or model compliance. A fresh session may be needed to observe changed discovery; verify it in the actual host rather than claiming it from hashes.

For a substantive guidance change, try a bounded task from the use-case table with an independent agent and raw source context, without supplying the expected answer. Include a neighboring negative case such as a prose-only correction. Evaluate selected guidance, source-backed decisions, preserved contracts and execution scope—not echoed headings or claimed compliance. Keep observations in the task/PR, not another permanent dated report. [OpenAI's skill-evaluation example](https://developers.openai.com/blog/eval-skills) motivates this distinction; one explicit-invocation rehearsal does not prove automatic discovery or universal adherence.

For a deliberate upstream upgrade, compare a reviewed pinned revision in a temporary destination, replace only the owned source/reference/prompt copies, refresh manifest hashes/license and rerun guidance checks. Do not track `main` automatically or edit marketplace caches. Keep application tests/builds and hardware checks out of prose-only verification.
