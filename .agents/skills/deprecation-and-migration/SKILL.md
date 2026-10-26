---
name: deprecation-and-migration
description: Plan or implement an explicitly requested SDK removal or API migration with consumer accounting, compatibility and rollback. Not routine refactoring, inactivity-based deletion or permission for deployments.
---

# Deprecation and Migration

> Local adaptation of Addy Osmani's pinned MIT skill; see the [license](references/addy-LICENSE.md) and [source manifest](../../../docs/engineering/skill-sources.json). Removal, usage evidence and pattern advice are narrowed below for this standalone SDK. Database-specific deployment guidance and unavailable cross-skill requirements are omitted. Apply the [canonical migration rules](../../../docs/engineering/conventions.md#11-module-design-and-explicit-migration); this skill grants no dependency, deployment, hardware, production-traffic or commit authority.

## Overview

Code is a liability, not an asset. Every line of code has ongoing maintenance cost — bugs to fix, dependencies to update, security patches to apply, and new engineers to onboard. Deprecation is the discipline of removing code that no longer earns its keep, and migration is the process of moving users safely from the old to the new.

Most engineering organizations are good at building things. Few are good at removing them. This skill addresses that gap.

## When to Use

Use only for an explicitly requested removal, deprecation decision or API migration, including a scoped consolidation that retires an existing interface. Routine cleanup, old filenames, low commit activity or a desire for a smaller diff do not authorize removal.

First account for Rust exports, Python aliases and keyword arguments, stubs/helpers, examples, tests, documentation, parent consumers and unknown external consumers. A repository search cannot prove a published SDK has no external users. Preserve compatibility aliases and wire fields until affected consumers and the supported deprecation window are accounted for; record unknowns rather than treating them as zero.

## Core Principles

### Code Is a Liability

Every line of code has ongoing cost: it needs tests, documentation, security patches, dependency updates, and mental overhead for anyone working nearby. The value of code is the functionality it provides, not the code itself. When the same functionality can be provided with less code, less complexity, or better abstractions, consider a scoped migration. Compatibility, safety coverage and an explicit removal decision still govern whether old code can go.

### Hyrum's Law Makes Removal Hard

With enough users, every observable behavior becomes depended on — including bugs, timing quirks, and undocumented side effects. This is why deprecation requires active migration, not just announcement. Users can't "just switch" when they depend on behaviors the replacement doesn't replicate.

### Deprecation Planning Starts at Design Time

When building something new, ask: "How would we remove this in 3 years?" Systems designed with clean interfaces and minimal surface area are easier to deprecate than systems that leak implementation details everywhere. Reuse existing compatibility seams; do not introduce feature flags, dependencies or frameworks just to satisfy this advice.

## The Deprecation Decision

Before deprecating anything, answer these questions:

```
1. Does this system still provide unique value?
   → If yes, maintain it. If no, assess an explicitly scoped removal; do not delete automatically.

2. How many users/consumers depend on it?
   → Quantify known consumers and record unknown external consumers separately.

3. Does a replacement exist?
   → If no, identify whether a replacement is required or the task explicitly sunsets the behavior. Do not invent replacement work or remove a depended-on capability.

4. What's the migration cost for each consumer?
   → Use authorized automation only after reviewing its effects. If manual and high-effort, weigh against maintenance cost.

5. What's the ongoing maintenance cost of NOT deprecating?
   → Security risk, engineer time, opportunity cost of complexity.
```

## Compulsory vs Advisory Deprecation

| Type | When to Use | Mechanism |
|------|-------------|-----------|
| **Advisory** | Migration is optional, old system is stable | Warnings, documentation, nudges. Users migrate on their own timeline. |
| **Compulsory** | An explicit release/compatibility decision requires removal after assessing security, maintenance and consumer impact | The supported release decision sets any deadline; provide proportionate migration support. |

**Default to advisory.** A compulsory removal or deadline needs an explicit compatibility/release decision, consumer accounting and proportionate migration support. Maintenance cost alone is not authority to break published aliases or force an external consumer cutover.

## The Migration Process

### Step 1: Build the Replacement

Where the task replaces behavior, verify the alternative covers the existing contract before retiring it. The replacement must:

- Cover all critical use cases of the old system
- Have documentation and migration guides
- Have relevant observed evidence within the permitted offline/native verification scope, with deployment and physical limits reported honestly; a production rollout is not a mandatory SDK check
- Preserve cancellation versus completion, explicit shutdown, buffer/FFI ownership and documented error/panic/unsafe contracts

### Step 2: Announce and Document

```markdown
## Deprecation Notice: OldService

**Status:** Deprecated as of 2025-03-01
**Replacement:** NewService (see migration guide below)
**Removal date:** Advisory — no hard deadline yet
**Reason:** OldService requires manual scaling and lacks observability.
            NewService handles both automatically.

### Migration Guide
1. Replace `import { client } from 'old-service'` with `import { client } from 'new-service'`
2. Update configuration (see examples below)
3. Run the owning repository's applicable offline regression checks (no new installer or tool implied)
```

### Step 3: Migrate Incrementally

Migrate consumers one at a time, not all at once. For each consumer:

```
1. Identify all touchpoints with the deprecated system
2. Update to use the replacement
3. Verify behavior matches (tests, integration checks)
4. Remove references to the old system
5. Confirm no regressions
```

**The Churn Rule:** Own the migration of known consumers within task scope, or provide compatible aliases/adapters and concrete migration documentation. Unknown external consumers remain a compatibility risk; do not interpret lack of local references as completed migration.

### Step 4: Remove the Old System

Only after the explicitly authorized removal accounts for affected consumers and the supported compatibility window:

```
1. Review source/exports/stubs/examples and permitted offline evidence; record external unknowns
2. Preserve aliases/adapters while compatibility obligations or unresolved consumers remain
3. Remove only the approved obsolete code/configuration
4. Consolidate tests only after proving equivalent safety and behavior coverage
5. Update current documentation; retain migration/history notes that supported consumers still need
```

Retain focused codec, malformed-input, cancellation/completion, shutdown and FFI regressions. Do not delete associated tests merely because an implementation is retired.

## Migration Patterns

### Strangler Pattern

When the task and environment permit, compare old/new behavior incrementally with isolated offline fixtures or fakes. The following traffic percentages are an illustrative service rollout, not an instruction to run production traffic simulation, deploy a service or connect a robot. Zero observed traffic alone cannot prove external SDK consumers have migrated.

```
Phase 1: New system handles 0%, old handles 100%
Phase 2: New system handles 10% (canary)
Phase 3: New system handles 50%
Phase 4: New system handles 100%, old system idle
Phase 5: Remove old system
```

### Adapter Pattern

Create an adapter that translates calls from the old interface to the new implementation. Consumers keep using the old interface while you migrate the backend.

```typescript
// Adapter: old interface, new implementation
class LegacyTaskService implements OldTaskAPI {
  constructor(private newService: NewTaskService) {}

  // Old method signature, delegates to new implementation
  getTask(id: number): OldTask {
    const task = this.newService.findById(String(id));
    return this.toOldFormat(task);
  }
}
```

### Feature Flag Migration

Only when an existing scoped deployment already has an appropriate flag mechanism, a flag can switch consumers incrementally. This illustration does not authorize adding flags, dependencies or a framework to the SDK:

```typescript
function getTaskService(userId: string): TaskService {
  if (featureFlags.isEnabled('new-task-service', { userId })) {
    return new NewTaskService();
  }
  return new LegacyTaskService();
}
```

## Zombie Code

Zombie code is code that nobody owns but everybody depends on. It's not actively maintained, has no clear owner, and accumulates security vulnerabilities and compatibility issues. Signs:

- Reported ownership or maintenance gaps with active consumers; elapsed commit time alone is not evidence of obsolescence
- No assigned maintainer or team
- Failing tests that nobody fixes
- Dependencies with known vulnerabilities that nobody updates
- Documentation that references systems that no longer exist

**Response:** Report evidence and identify ownership. A reviewed maintenance or explicitly requested deprecation plan can follow; inactivity, legacy naming, failing tests or missing owners do not themselves authorize removal or dependency changes.

## Common Rationalizations

| Rationalization | Reality |
|---|---|
| "It still works, why assess it?" | Compare demonstrated maintenance cost with compatibility and safety value; a working legacy API can remain the correct choice. |
| "There are no local callers" | Published SDK consumers can be external and unknown. Preserve supported aliases or obtain an explicit removal decision that accounts for that risk. |
| "The migration is too expensive" | Compare actual migration/compatibility costs with maintenance costs; neither outcome is automatic. |
| "We'll deprecate it after we finish the new system" | Deprecation planning starts at design time. By the time the new system is done, you'll have new priorities. Plan now. |
| "Users will migrate on their own" | They won't. Provide tooling, documentation, and incentives — or do the migration yourself (the Churn Rule). |
| "We can maintain both systems indefinitely" | Two systems doing the same thing is double the maintenance, testing, documentation, and onboarding cost. |

## Red Flags

- Replacement migrations without an adequate alternative or an explicit intentional-sunset decision
- Deprecation announcements with no migration tooling or documentation
- "Soft" deprecation that's been advisory for years with no progress
- Zombie code with no owner and active consumers
- Unplanned feature work added to a deprecated system without considering supported consumers and the migration decision
- Deprecation without measuring current usage
- Removing code without an explicit compatibility decision and consumer evidence, including external unknowns

## Verification

Before claiming an authorized migration/removal complete:

- [ ] Known Rust/Python consumers, aliases, kwargs, exports, stubs, helpers, examples and docs are accounted for; external unknowns are explicit
- [ ] Replacement behavior or intentional sunset scope is documented, with compatibility window and rollback path
- [ ] Relevant offline regressions preserve wire values, error contracts, cancellation/completion, shutdown and FFI ownership; meaningful internal safety tests remain
- [ ] Old code/configuration is removed only within the explicit decision; compatibility aliases/adapters and needed migration notices remain until their obligations end
- [ ] Observed checks and omissions are distinguished from production, deployment and physical validation

Use the owning repository's existing application gates when application code changes are authorized. Guidance-only work uses static metadata, provenance, reference and diff checks; it does not run application checks. Do not install an updater/migration framework, change dependencies, commit or run hardware merely because a pattern above mentions migration support.
