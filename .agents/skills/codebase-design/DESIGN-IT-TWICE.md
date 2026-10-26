# Design It Twice

> Local adaptation: optional, proportionate interface comparison using available native agents. No fixed agent count, recursive orchestration, missing glossary dependency or new approval/commit authority. See [SKILL.md](SKILL.md) for canonical SDK rules and provenance.

When the user wants to explore alternative interfaces for a chosen deepening candidate, consider this comparison pattern when alternatives would resolve a meaningful design uncertainty. Based on "Design It Twice" (Ousterhout): your first idea is unlikely to be the best.

Uses the vocabulary in [SKILL.md](SKILL.md): **module**, **interface**, **seam**, **adapter**, **leverage**.

## Process

### 1. Frame the problem space

Before comparing designs, briefly frame the problem space for the chosen candidate:

- The constraints any new interface would need to satisfy
- The dependencies it would rely on, and which category they fall into (see [DEEPENING.md](DEEPENING.md))
- A rough illustrative code sketch to ground the constraints, not a proposal, just a way to make the constraints concrete

Keep the brief proportionate and continue clear authorized work without a new permission handoff. Preserve known callers, public aliases, wire fields, cancellation/completion ownership and FFI error/panic/safety contracts in every alternative.

### 2. Explore alternatives proportionately

For a small decision, compare alternatives directly. For independent nontrivial alternatives, use the smallest useful number of available native sub-agents within the leader's authorization. Do not spawn recursively without authorization. Each alternative should be **meaningfully different**, not a cosmetic rename.

When delegating, give each agent bounded read-only ownership and a separate technical brief (file paths, coupling details, dependency category from [DEEPENING.md](DEEPENING.md), what sits behind the seam). The brief is independent of the user-facing problem-space explanation in Step 1. Give each agent a different design constraint:

- Minimal surface: "Reduce caller complexity while preserving every required operation, alias and error contract."
- Flexibility: "Maximise flexibility: support many use cases and extension."
- Common caller: "Optimise for the most common caller: make the default case trivial."
- Adapter boundary (if applicable): "Design around ports & adapters for cross-seam dependencies."

Include [SKILL.md](SKILL.md), the relevant SDK conventions, existing architecture vocabulary, actual callers and allowed evidence in the brief. Do not require a nonexistent glossary or rename existing services/ports merely to match these terms.

Each comparison or delegated design covers:

1. Interface (types, methods, params, plus invariants, ordering, error modes)
2. Usage example showing how callers use it
3. What the implementation hides behind the seam
4. Dependency strategy and adapters (see [DEEPENING.md](DEEPENING.md))
5. Trade-offs: where leverage is high, where it's thin

### 3. Present and compare

Present the relevant alternatives concisely, then compare them in prose. Contrast by **depth** (leverage at the interface), **locality** (where change concentrates), and **seam placement**.

After comparing, give your own recommendation: which design you think is strongest and why. If elements from different designs would combine well, propose a hybrid. Be opinionated: the user wants a strong read, not a menu.
