# Deepening

> Local adaptation: preserve established SDK ownership, lifecycle/FFI contracts and meaningful safety regressions; no automatic test deletion or minimum adapter count. See [SKILL.md](SKILL.md) for canonical project rules and provenance.

How to deepen a cluster of shallow modules safely, given its dependencies. Assumes the vocabulary in [SKILL.md](SKILL.md): **module**, **interface**, **seam**, **adapter**.

## Dependency categories

When assessing a candidate for deepening, classify its dependencies. The category determines how the deepened module is tested across its seam.

### 1. In-process

Pure computation, in-memory state, no I/O. Consider merging when the responsibilities are cohesive; test through the resulting interface while preserving independently useful invariants. No adapter is required merely to create one.

### 2. Local-substitutable

Dependencies with existing local test stand-ins (for example an in-memory transport). Use a suitable stand-in to exercise behavior offline. Preserve a port where ownership, safety or actual callers justify it; do not add dependencies or force a port removal to fit this category.

### 3. Remote but owned (Ports & Adapters)

Your own services across a network boundary (microservices, internal APIs). Reuse an existing **port** (interface) where available. The deep module owns the logic; transport is injected as an **adapter**. Tests use a suitable offline adapter. This category does not require creating services or a new transport framework.

Recommendation shape: *"Define a port at the seam, implement an HTTP adapter for production and an in-memory adapter for testing, so the logic sits in one deep module even though it's deployed across a network."*

### 4. True external (Mock)

Third-party services (Stripe, Twilio, etc.) you don't control. The deepened module takes the external dependency as an injected port; tests provide a mock adapter.

## Seam discipline

- **Justify the seam, not an adapter quota.** Existing protocol/FFI safety, ownership and offline-test boundaries can warrant a port with one current adapter. A production/test pair can be useful evidence, not a required count. Reuse existing ports and concrete collaborators; avoid indirection without a demonstrated responsibility.
- **Internal seams vs external seams.** A deep module can have internal seams (private to its implementation, used by its own tests) as well as the external seam at its interface. Don't expose internal seams through the interface just because tests use them.

## Testing strategy: preserve evidence before consolidation

- Characterize preserved behavior before extraction. Remove or consolidate a test only after showing equivalent or stronger coverage of its assertions and failure modes; a new facade test alone is not proof.
- Add caller-observable tests at the deepened interface and retain relevant integration coverage.
- Keep focused internal tests for codecs, malformed-input bounds, cancellation/completion, shutdown, FFI ownership and other safety invariants when they expose failures more directly. Private placement does not make a test disposable.
- Prefer behavior assertions that survive harmless refactors; inspect why a test changes rather than treating any internal coupling as a removal mandate. No application tests are run merely to validate this guidance.
