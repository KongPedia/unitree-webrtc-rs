---
name: codebase-design
description: Design or review module interfaces, responsibility boundaries and testable seams for a scoped SDK design or refactor. Not a requirement for prose-only work or a public API rename.
---

# Codebase Design

> Local adaptation of Matt Pocock's pinned MIT skill; see the [license](references/matt-LICENSE.md) and [source manifest](../../../docs/engineering/skill-sources.json). Vocabulary, seam/test advice and design comparison are adapted below to preserve SDK compatibility, safety coverage and proportionate native delegation. Apply the [canonical SDK conventions](../../../docs/engineering/conventions.md#11-module-design-and-explicit-migration) with this skill.

Design **deep modules**: a lot of behaviour behind a small interface, placed at a clean seam, testable through that interface. Use this language and these principles wherever code is being designed or restructured. The aim is leverage for callers, locality for maintainers, and testability for everyone.

## Glossary

Use these terms to make design trade-offs precise, alongside the SDK's established protocol, service, port, bridge and API vocabulary. Do not rename modules, public aliases or wire fields merely to match the glossary.

**Module**: anything with an interface and an implementation. Deliberately scale-agnostic: a function, class, package, or tier-spanning slice. Existing component/service names remain valid.

**Interface**: everything a caller must know to use the module correctly: the type signature, but also invariants, ordering constraints, error modes, required configuration, and performance characteristics. A type signature alone is narrower than this meaning; existing API terminology remains valid.

**Implementation**: what's inside a module, its body of code. Distinct from **Adapter**: a thing can be a small adapter with a large implementation (a Postgres repo) or a large adapter with a small implementation (an in-memory fake). Reach for "adapter" when the seam is the topic; "implementation" otherwise.

**Depth**: leverage at the interface. The amount of behaviour a caller (or test) can exercise per unit of interface they have to learn. A module is **deep** when a large amount of behaviour sits behind a small interface, **shallow** when the interface is nearly as complex as the implementation.

**Seam** _(Michael Feathers)_: a place where you can alter behaviour without editing in that place; the *location* at which a module's interface lives. Where to put the seam is its own design decision, distinct from what goes behind it. Distinguish this replaceability sense from dependency, resource and FFI safety boundaries; retain established terminology.

**Adapter**: a concrete thing that satisfies an interface at a seam. Describes *role* (what slot it fills), not substance (what's inside).

**Leverage**: what callers get from depth. More capability per unit of interface they learn. One implementation pays back across N call sites and M tests.

**Locality**: what maintainers get from depth. Change, bugs, knowledge, and verification concentrate in one place rather than spreading across callers. Fix once, fixed everywhere.

## Deep vs shallow

**Deep module** = small interface + lots of implementation:

```
┌─────────────────────┐
│   Small Interface   │  ← Few methods, simple params
├─────────────────────┤
│                     │
│  Deep Implementation│  ← Complex logic hidden
│                     │
└─────────────────────┘
```

**Shallow module** = large interface + little implementation (avoid):

```
┌─────────────────────────────────┐
│       Large Interface           │  ← Many methods, complex params
├─────────────────────────────────┤
│  Thin Implementation            │  ← Just passes through
└─────────────────────────────────┘
```

When designing an interface, ask:

- Can I reduce the number of methods?
- Can I simplify the parameters?
- Can I hide more complexity inside?

## Principles

- **Depth is a property of the interface, not the implementation.** A deep module can be internally composed of small, mockable, swappable parts; they just aren't part of the interface. A module can have **internal seams** (private to its implementation, used by its own tests) as well as the **external seam** at its interface.
- **The deletion test.** Imagine deleting the module. If complexity vanishes, it was a pass-through. If complexity reappears across N callers, it was earning its keep.
- **The interface is a useful test surface.** Prefer caller-observable behavior while retaining focused internal tests for codecs, malformed input, cancellation/completion, shutdown and FFI invariants. Internal coverage is not by itself evidence of a bad shape.
- **Justify a seam by its responsibility and evidence.** A production/test adapter pair can demonstrate variation, but no fixed adapter count is required. Existing protocol/FFI isolation or safety and offline-test needs can justify a seam; avoid speculative traits and pass-through layers.

## Designing for testability

Good interfaces make testing natural:

1. **Accept dependencies, don't create them.**

   ```typescript
   // Testable
   function processOrder(order, paymentGateway) {}

   // Hard to test
   function processOrder(order) {
     const gateway = new StripeGateway();
   }
   ```

2. **Prefer results for pure policy; isolate required side effects.**

   ```typescript
   // Testable
   function calculateDiscount(cart): Discount {}

   // Hard to test
   function applyDiscount(cart): void {
     cart.total -= discount;
   }
   ```

3. **Small surface area.** Fewer methods and parameters can simplify callers and test setup; required behavior and safety coverage do not shrink merely because the interface does.

These pure-function examples do not forbid necessary transport effects. Keep side-effect ownership, cancellation, completion, errors and cleanup explicit; preserve public error/panic and unsafe precondition documentation.

## Relationships

- A **Module** has an **Interface** comprising the surfaces it presents to callers and tests, including preserved compatibility aliases.
- **Depth** is a property of a **Module**, measured against its **Interface**.
- A **Seam** is where a **Module**'s **Interface** lives.
- An **Adapter** sits at a **Seam** and satisfies the **Interface**.
- **Depth** produces **Leverage** for callers and **Locality** for maintainers.

## Rejected framings

- **Depth as ratio of implementation-lines to interface-lines** (Ousterhout): rewards padding the implementation. We use depth-as-leverage instead.
- **"Interface" as the TypeScript `interface` keyword or a class's public methods**: too narrow: interface here includes every fact a caller must know.
- **Interchangeable boundary meanings**: clarify whether the topic is replaceability, ownership, protocol or FFI safety rather than banning established boundary vocabulary.

## Going deeper

- **Deepening a cluster given its dependencies**, see [DEEPENING.md](DEEPENING.md): dependency categories, seam discipline and evidence-based test consolidation.
- **Exploring alternative interfaces**, see [DESIGN-IT-TWICE.md](DESIGN-IT-TWICE.md): compare meaningfully different designs on depth, locality and seam placement; use bounded native sub-agents only when useful and authorized.
