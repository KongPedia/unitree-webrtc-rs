# Engineering conventions

These are project-local design decisions, not a demand to apply every SOLID or GoF pattern. Read current source before relying on a review. The aim is a maintainable Rust-owned transport with a compatible thin Python API, bounded latency/memory, and evidence-based robot support.

## 1. Responsibility and dependency direction

| Area | Owns | Does not own |
| --- | --- | --- |
| `protocol/` | Wire envelopes, request identity, ports, documented protocol constants | Python runtime, HTTP clients, codecs |
| `connection/` | Validated session/resource owner, adapter assembly and connect/disconnect/retry lifecycle | NumPy conversion, model-specific command catalogues |
| `datachannel/core/` | Routing, subscription/request completion, validation/heartbeat orchestration | All future robot application features |
| `datachannel/lidar/` | Metadata validation and point-cloud decoding/work scheduling | Python callbacks and connection authentication |
| `infrastructure/` | HTTP signaling, security transforms, RTC adapter, platform setup | User-facing Python API policy |
| `audio/`, `video/` | Media decode/encode, RTP handling and owned buffers | Python callback registry |
| `interface/` | Python conversion/errors/aliases and callback dispatch | Duplicate transport/session policy or resource assembly |

Existing ports-and-adapters are useful: services depend on the contracts in `protocol/ports.rs`; concrete HTTP/RTC adapters are wired in `connection::UnitreeSession`. Python delegates to this Rust owner. Do not add a second service layer solely to rename the existing one.

SRP means **one reason to change**, not one file per function or an arbitrary line limit. Split a module when independent policies (for example chunk reassembly and heartbeat) can be tested separately. Tests colocated in a large file are not themselves evidence of SRP failure. A composition root may assemble many types without becoming a domain service.

## 2. Patterns to use only when they solve a concrete variation

| Pattern | Appropriate use here | Avoid |
| --- | --- | --- |
| Ports and adapters / dependency inversion | Existing `SignalingPort`, `RtcEnginePort`, `DataChannelPort`; fake transport for offline tests | Traits for every private helper |
| Adapter / façade | PyO3 translates Rust results and legacy Python aliases | Auth/decode logic duplicated in bindings |
| Strategy + capability/profile data | Verified model/firmware differences in signaling, auth, commands or codecs | `if model == ...` across router, codecs and bridges |
| Explicit state machine | Transport-open, authenticated/validated, closing and disconnected transitions; centralized rollback | Independent booleans that can disagree; elaborate typestate rewrite without a demonstrated need |
| Owned resource / RAII | Synchronous final cleanup; explicit async `shutdown`/`disconnect` with cancellation and task/thread joining | Assuming async cleanup can simply be awaited in `Drop`; `Arc` cycles without a shutdown protocol |
| Newtype / error enum | Units, request/session identity or actionable error categories where confusion already occurs | Wrapping every string, adding an error dependency for a few variants |

For a few built-in verified profiles, an enum and immutable data can suffice. Add a trait-based plugin boundary only if an actual independently supplied implementation needs it. Default to the existing utilities, standard library and installed dependencies.

## 3. Naming and public compatibility

| Construct | Convention | Example |
| --- | --- | --- |
| Rust modules/files/functions/methods/locals | `snake_case` | `wait_datachannel_open`, `session_id` |
| Rust types/traits/enum variants | `UpperCamelCase`; acronyms as words | `RtcEngine`, `RequestIdentity`, `LocalSta` for a new internal variant |
| Rust constants/statics | `SCREAMING_SNAKE_CASE` | `MAX_DECOMPRESSED_BYTES` |
| New Python functions/arguments | `snake_case` | `serial_number`, `on_frame` |
| Python classes | `UpperCamelCase` | `UnitreeWebRTCConnection` |
| Boolean / unit-bearing policy | State the predicate/unit | `is_connected`, `timeout_secs`, `capacity_frames` |

- **Compatibility exception:** existing `LocalSTA`, `LocalAP`, `serialNumber`, `connectionMethod`, `disableTrafficSaving`, `publish_request_new`, exported dictionaries and wire keys remain valid. Do not mass-rename public APIs to make a lint happy. New aliases require documented precedence and a deprecation/migration plan if retiring old ones.
- Name a `Service` for its policy, a `Bridge` for an FFI adapter, a `Port` for an actual external boundary and a `Codec` for a transformation. Prefer precise names over `Manager`, `Utils`, `Helper`, `New` and `Old` for new abstractions; existing legacy names need caller evidence before removal.
- Name files for the responsibility they contain, not a design-pattern label: `connection/session.rs` owns `UnitreeSession`, `connection/lifecycle.rs` owns connect/validation/rollback/shutdown procedures. A façade describes the API shape, not the session's name.
- Keep dtype/shape metadata consistent across Rust buffers, Python callback contracts, stubs, examples and README. For example, `Vec<f32>` transferred to NumPy is not `float64`.
- Rust naming and formatting are partly covered by compiler/Clippy/rustfmt. Existing Ruff hooks cover Python lint/format, not a complete naming/architecture policy. No new automatic rule enforcement is claimed by this document.

## 4. Shallow control flow without hiding complexity

The maintainer prefers few nested `if`/`for` blocks. Optimize for explicit responsibilities and a small number of policy decisions, not a token-count ban.

| Situation | Prefer | Do not do |
| --- | --- | --- |
| Invalid input / missing optional value | Guard clause, `?`, `ok_or_else`, `let ... else` | Deep success-path nesting or removal of validation |
| Known protocol states/message kinds | Enum + exhaustive `match`, parsed once at the boundary | Repeated string guards in several layers; duplicate enum/map catalogues |
| Fixed lookup data | One authoritative table/map | A long condition chain or a separate strategy type per constant |
| Transform/filter/search a sequence | Readable `map`, `filter`, `find`, `any`, `collect` or `try_fold` | Long iterator chains with hidden side effects or extra allocations |
| Verified behavior varies by model/protocol | Composition/strategy at the existing adapter boundary | `if model == ...` in every service; a speculative plugin registry |
| Repeated schema handling | Normalize into one internal value then a shared parser/assembler | Duplicated framing/chunk logic or helpers that only conceal branches |
| Codec/event/worker iteration | A clear loop with bounds and cancellation | Removing an efficient necessary loop solely to avoid `for`/`while` |

Prefer enum/data for a closed set; use traits when a real independently varying implementation justifies them. Naming more classes is not SRP. A `match` that centralizes a genuine decision is fine; changing syntax without improving contracts is not a refactor.

## 5. Constants, configuration and multi-model support

Separate three categories:

1. **Protocol facts:** topic names, API IDs, framing values, firmware auth schemes. Keep immutable and traceable to upstream revision/captured sanitized fixture. Configuration does not make a wrong wire value extensible.
2. **Runtime policy:** timeouts, queue budgets, decoder preference and retry settings. Give safe documented defaults; expose overrides only for demonstrated deployment needs, with units/range validation.
3. **Device/user data:** address, model/firmware identity, credentials, per-device key. Never commit or log secrets; per-connection values must not accidentally become process-wide singleton state.

Keep verified model/firmware differences at the existing adapter boundary; no RobotProfile API is currently implemented. Require a source-backed capability matrix and offline fixtures before advertising another model. Distinguish declared profile, capability negotiated from the device, fixture-tested support and physical validation; mark unknown capabilities unsupported in high-level helpers. A raw expert pub/sub API can remain available without falsely promising semantic support. Do not identify models by undocumented serial-number guesses.

Model expansion and multi-session scale are different: validate independent credentials/subscriptions/request IDs, per-session resource limits, total thread/memory growth and cancellation. Do not multiply worker pools by every CPU core for every robot without measurement.

## 6. Ownership, concurrency and Python boundary

- Write down who owns each buffer and sender, what closes it, whether overflow blocks/drops/errors, and how shutdown interrupts a wait. Choose reliability for control/RPC independently of latest-frame freshness for video/LiDAR.
- Do not hold a mutex during Python user callbacks or hold synchronous locks across `.await`. Do not block a Tokio worker on synchronous channel sends when a nonblocking/async boundary is needed.
- Keep request correlation, expected response and session generation protections. On cancellation, timeout, disconnect and reconnect, pending requests need a deterministic outcome and bounded retained state.
- Validate packet/header sizes and decompressed size ceilings, chunk order/count/expiry, numeric conversions, finite timeouts and URI schemes before allocation or pipeline construction. Use checked arithmetic and UTF-8-safe/byte-level parsing as appropriate.
- Log actionable errors with fixed event/category identifiers, numeric session/registration context and redaction; do not echo arbitrary peer topics. A repeated recoverable callback error should be observable without killing transport; swallowing every error is not resilience.
- Heavy Rust work and blocking waits should not keep Python attached unnecessarily. Python object access still requires PyO3 attachment. `Python::detach` does not make shared data race-free.
- `unsafe impl Send/Sync` needs a `SAFETY` rationale covering actual library thread affinity and all calling paths. The extension and legacy driver explicitly declare GIL use; this support-policy opt-out is not evidence of a discovered race. Do not remove it based only on ordinary GIL-path tests or PyO3 defaults.
- Scope a zero-copy claim to the specific owned-buffer handoff. Document array dtype, dimensions, strides, mutation and lifetime (including retention after disconnect). Avoid borrowed GStreamer/decoder memory unless the Python owner demonstrably keeps it alive.

## 7. Change and verification contract

Before editing, list affected callers, public exports, `.pyi`, helper models, examples, fixtures, manifests and docs. For a refactor, state the smallest responsibility boundary to extract and the behavior/ABI/wire format preserved. Add regression coverage at the common failure layer, not only in one caller.

| Change | Minimum relevant evidence |
| --- | --- |
| Protocol/parser | Positive and malformed-input offline fixtures; bounds/errors; known wire compatibility |
| Session/queue | Failed-connect rollback, reconnect epoch, cancellation, deterministic teardown and overflow behavior |
| PyO3/NumPy | Actual native import/call, aliases/errors, dtype/shape/ownership/lifetime; mocks alone are insufficient |
| Robot profile | Positive and unsupported capabilities, sanitized model/firmware fixtures, explicit hardware gap |
| Performance | Reproducible workload, platform/build versions, throughput and p95/p99 latency, memory/drops; baseline comparison |
| Packaging | Fresh wheel install/import outside repo, shipped stubs/helpers, supported-platform native dependencies |
| Instructions/docs | Metadata parsing, reference resolution, realistic bounded skill scenario and diff/conflict inspection |

For application work inspect current `Makefile` before selecting `make check`, `make test-rust`, `make build` and `make test-python`. Rust tests link Python; extension builds use the explicit `extension-module` feature. `build.rs` uses PyO3's target-specific linker helpers, and the test target supplies the embedded interpreter environment; this is not proof of Linux/Jetson portability. Rebuild the extension after Rust changes before native Python tests. Native media dependencies are prerequisites, not a reason to claim an unrun check passed. Docs-only work does not run those targets.

## Sources

These references support specific decisions, not installation of a new framework:

- [Rust API naming guidelines](https://rust-lang.github.io/api-guidelines/naming.html) and [API checklist](https://rust-lang.github.io/api-guidelines/checklist.html): idiomatic names, compatibility, documentation and API design considerations.
- [Strategy, Rust Design Patterns](https://rust-unofficial.github.io/patterns/patterns/behavioural/strategy.html): community-maintained pattern reference, not an official Rust requirement.
- [PyO3 0.28 parallelism](https://pyo3.rs/v0.28.0/parallelism) and [free-threading](https://pyo3.rs/v0.28.0/free-threading.html): attachment/detachment and thread-safety audit obligations.
- [rust-numpy 0.28 IntoPyArray](https://docs.rs/numpy/0.28.0/numpy/convert/trait.IntoPyArray.html) and [PyArray](https://docs.rs/numpy/0.28.0/numpy/array/struct.PyArray.html): owned Rust memory handoff, including resizing limitations.
- [Rustonomicon Send and Sync](https://doc.rust-lang.org/nomicon/send-and-sync.html): unsafe thread-safety contracts.
- [Maturin project layout](https://www.maturin.rs/project_layout.html): Python packages and stubs must match configured packaging layout.

## Implemented transport safety policies

See [resource-policy.md](resource-policy.md) for named runtime limits, worker ownership, tightened input contracts, Rust codec migration, and the boundary-specific zero-copy claim. Current source and focused tests remain the implementation evidence.

## 8. Logging and acceptance policy

- Reuse **tracing/tracing-subscriber**. Do not introduce a second logger framework, Python hot-path forwarding or an unbounded async log queue for this SDK. Rust library users own subscriber setup; Python import offers a WARN/stderr fallback, disabled by `UNITREE_WEBRTC_RS_LOG_INIT=0`. Existing host subscribers are never replaced.
- Configure before import. SDK-owned invalid explicit settings return `ValueError`; disabled setup/host ownership does not parse unused SDK filters. No runtime reload API is implied.
- ERROR means terminal operation/stream failure and must accompany actual failure state or termination. WARN means recoverable degradation/loss; INFO is meaningful lifecycle outcome; DEBUG is progress/selection/statistics. Keep safe typed failure categories rather than either raw secret-bearing error strings or uninformative generic failures.
- Repeated drops/malformed input/callback failures use a fixed-key **owner-local** `RepeatLog`: first warning, then event-driven five-second summaries, with teardown best-effort flush. Reset aggregation between epochs. Do not key unlimited maps by peer topics/error text. Terminal errors and first occurrences are not hidden by throttling.
- Never log raw payloads, SDP, credentials, arbitrary Python exception text/class names, or unvalidated addresses/URLs. Callback diagnostics use numeric registration IDs, fixed builtin type-identity classifications and stage/generation/counts. Source-owned enum classifications are safe to retain.
- Rate limits cover selected SDK repeat paths, not all host/plugin/dependency output. Python logging/print, native GStreamer debug and disk rotation remain distinct ownership boundaries. Queued media/accounting quotas do not limit user-retained NumPy arrays or total RSS.
- Use `make acceptance` for source changes; it serializes guidance/full-feature lint/core/default Rust/fresh-native checks within one Make invocation. `make test-python` rebuilds the extension. Independent parallel Cargo invocations that switch PyO3 modes still require coordination.
- Guidance changes use `make check-guidance` (metadata, pinned SHA-256, local path links). Release workflow runs acceptance and fresh-wheel smoke before upload; a workflow configuration is not an executed CI result. File-count/hash checks are not proof that LLM automatic skill selection always works.
- Tests asserting emitted logs must consider process-global tracing callsite interest, not only scoped subscribers. If a production callsite is shared with untraced parallel tests, use the test-only exact-process isolation helper; retain positive-output and redaction assertions. Do not hide this race with sleeps, retries or whole-suite serialization.

User-facing configuration is in [README logging](../../README.md#logging-from-python); runtime memory/queue boundaries are in [resource-policy.md](resource-policy.md).

## 9. Rust-specific decisions for coding agents

Read this adaptation before applying the pinned generic skills. They supply concepts, not code validated against this crate's toolchain, dependencies or resource contracts.

- **Check the actual build contract.** Use [rust-toolchain.toml](../../rust-toolchain.toml), [Cargo.toml](../../Cargo.toml) and [Cargo.lock](../../Cargo.lock), not a prompt's “Rust 1.75+” or “latest” label. The pinned compiler is not a declared minimum supported Rust version. Do not change edition, features, dependencies or use nightly-only APIs to imitate an example. Consult version-matching API docs; a 0.28-series PyO3 guide is not patch-specific verification.
- **Choose Rust-shaped boundaries.** Prefer existing structs, enums, functions and owned values; add a trait for a real replaceable boundary, not a Python class hierarchy. Borrow inputs for synchronous helpers when ownership is unnecessary; transfer owned data at task/FFI boundaries when its lifetime requires it. Do not silence borrow errors with blanket `clone`, `Arc<Mutex<_>>`, leaks or `unsafe`. Add identity/unit types only for demonstrated ambiguity; preserve established aliases and wire fields.
- **Preserve dispatch and failure contracts.** [protocol/ports.rs](../../src/protocol/ports.rs) uses explicit boxed `Send` futures. Native `async fn` in a trait is not automatically `dyn`-compatible; preserve lifetime/`Send` requirements and inspect consumers before changing its shape or adding `async-trait`. Keep existing `PortResult`/FFI errors compatible instead of introducing an error framework by default. New public behavior should document relevant errors, panic conditions and unsafe preconditions; external-input errors must not become `unwrap` panics.
- **Review the kind of lock, not a slogan.** Keep short synchronous state locks out of suspension and user callbacks. An async mutex guard intentionally held across `.await` can serialize lifecycle cleanup, as in [ConnectionAttempt](../../src/connection/service.rs); inspect lock order, cancellation and reentrancy before removing it. Do not replace every synchronous mutex with an async mutex.
- **Distinguish cancellation from completion.** Dropping a Tokio `JoinHandle` detaches its task; dropping a directly polled future does not roll back prior effects. For each timeout/`select!` branch identify what is dropped, what keeps running and who retains cleanup/completion ownership. Notify workers and observe completion where required; cancel-plus-sleep is not a join. Existing best-effort final `Drop` limits remain as documented in [resource policy](resource-policy.md), not a universal teardown guarantee.
- **Bound work before spawning it.** A `JoinSet` tracks tasks but does not impose concurrency/byte limits. Reuse the existing worker/admission owner. If `spawn_blocking` is justified, account for queued/running work and cooperative shutdown: a started blocking closure cannot be stopped by `abort`. Do not move long-lived codec loops into per-frame blocking tasks. Do not let a non-`'static` pool borrow escape into a spawned task from `Drop`; a deferred pool return must restore resource availability before releasing its pool permit.
- **Keep Python attachment distinct from synchronization.** Follow the existing FFI ownership rules in section 6. Attachment permits Python access; detachment is not a cancellation, thread-safety or free-threaded compatibility guarantee. A native callback test must exercise conversion/ownership and a failure path, not merely use a mock with the same method names.
- **Test observable contracts, not the implementation's wording.** Derive expected wire values from an independent fixture/specification and assert meaningful outcomes for malformed input, overflow, stale epochs or cancellation. Avoid copying the implementation's branches into the expected result or checking that a constant equals itself. For a repair, choose a regression that would fail for the plausible broken behavior; claims of RED/GREEN require actual execution.

### Sources and limits

- [Rust API Guidelines checklist](https://rust-lang.github.io/api-guidelines/checklist.html) and [Rust Reference: dyn compatibility](https://doc.rust-lang.org/reference/items/traits.html#dyn-compatibility): naming, type/API/failure/safety contracts and trait-dispatch constraints; not a reason to break compatibility for cosmetic consistency.
- [Microsoft's Rust AI guidelines](https://microsoft.github.io/rust-guidelines/guidelines/ai/) and [agent resources](https://microsoft.github.io/rust-guidelines/agents/index.html): useful Rust-native design and meaningful-test guidance, not Rust language requirements. Read relevant sections on demand, not the entire combined manual. Its single-public-path preference does not override compatibility exports; avoiding design diaries in user-facing docs does not prohibit the maintained [architecture map](../architecture_analysis.md).
- [Tokio shutdown](https://tokio.rs/tokio/topics/shutdown), [Mutex](https://docs.rs/tokio/1.50.0/tokio/sync/struct.Mutex.html), [JoinHandle](https://docs.rs/tokio/1.50.0/tokio/task/struct.JoinHandle.html) and [spawn_blocking](https://docs.rs/tokio/1.50.0/tokio/task/fn.spawn_blocking.html): lifecycle/locking semantics for the locked Tokio version. Tutorial channels and delay-based illustrations do not replace this SDK's admission/completion contracts.
- [PyO3 0.28 free-threading](https://pyo3.rs/v0.28.0/free-threading.html): series-level attachment and safety context; inspect the locked API/source for patch-specific behavior.

## 10. External Rust guides and project precedence

The local [Rust best practices skill](../../.agents/skills/rust-best-practices/SKILL.md) complements the existing async and architecture skills. Use selected chapters for the changed seam, not a new mandatory framework. Examples are reference knowledge, not verified code for this SDK.

- **Keep the current build and error contracts.** An upstream `allowed-tools` list or Cargo command is not execution authority. Use the feature-aware Makefile checks when application execution is in scope; do not blindly run `--all-features` across the PyO3 extension/test modes. Do not introduce `thiserror`, `anyhow`, `insta`, nightly tools, crate-wide lint bans or a new test framework just to match examples.
- **Treat generic thresholds and patterns as heuristics.** A fixed byte-count rule for `Copy`, one-assertion-per-test, generics over `dyn`, typestate, and broad `expect`/clone prohibitions do not replace observed costs, meaningful boundary assertions or this crate's existing boxed-future/error/public API contracts. Do not split/coalesce tests or add traits merely to obey an illustrative rule.
- **Maintain Rust/FFI safety reasoning.** Follow sections 6 and 9 for owned buffers, Python attachment, cancellation/completion, locking and unsafe rationale. Stylistic guidance does not authorize removing a necessary shared owner or FFI panic-containment boundary, concealing a failure, or claiming zero-copy/free-threaded safety without evidence.

### Using Google's Rust Style Guide

The [official Google guide](https://google.github.io/styleguide/rustguide.html) is preserved as a [pinned local snapshot](../../.agents/skills/rust-best-practices/references/google-rust-style-guide.md), with its [license](../../.agents/skills/rust-best-practices/references/google-styleguide-LICENSE.md) and [attribution/licensing scope](../../.agents/skills/rust-best-practices/references/google-styleguide-licensing.md). It is design/style reference material, not Google's SDK endorsement or a new repository-wide lint policy. Read relevant headings when making Rust API/style/safety judgments.

- Apply useful general guidance contextually: flexible borrowed inputs (`&str`, slices), clear ownership/naming, standard traits where appropriate, explicit errors and safety/panic documentation, and readable pattern matching. Preserve established Python aliases, wire fields and Rust public compatibility; do not mass-rename APIs or add `non_exhaustive` solely for style.
- **Exclude platform-specific rules.** Chromium's async/await and standard-thread bans, flat-file/small-crate layout, import/build tooling and test framework do not apply to this Cargo/Tokio SDK. Fuchsia-specific mutex and test guidance also does not apply. Keep current modules, Tokio, standard/native workers, PyO3 macros and existing offline tests.
- **Shared ownership is contextual.** Google's `Arc`/`Rc` preference includes a complexity exception, and pointer cloning is not a deep copy. Keep justified async/FFI ownership; make lifecycle completion explicit rather than attempting a blanket ban or replacing it with unsafe/complex borrowing.
- **Do not import formatter/toolchain policy.** Retain `rust-toolchain.toml`, edition, Cargo locks and current rustfmt/Clippy configuration. Google's width/options, unstable examples and build-system conventions do not authorize tool upgrades or a formatting-only rewrite.

The snapshot's prose is CC-BY-3.0 and its code samples are Apache-2.0 per the upstream licensing declaration. Source identities, immutable revisions, original/local hashes and adaptations are recorded in [skill-sources.json](skill-sources.json); the original guide remains unchanged.

## 11. Module design and explicit migration

The selected [codebase-design](../../.agents/skills/codebase-design/SKILL.md) and [deprecation-and-migration](../../.agents/skills/deprecation-and-migration/SKILL.md) skills complement the rules above; neither mandates a new architecture or removal project.

- Use deep-module vocabulary to clarify responsibility and caller leverage, not to rename established services/ports, public aliases or wire fields. A safety/FFI/protocol seam can be justified without two adapters. Compare designs proportionately; a small choice does not require three or more agents or recursive delegation.
- Preserve caller-observable tests and useful internal codec, malformed-input, cancellation/completion, shutdown and FFI regressions. Consolidate tests only with evidence of equivalent or stronger coverage; deepening is not authority to delete shallow-unit tests automatically. Keep explicit error/panic/unsafe documentation and resource completion ownership.
- Use migration guidance only for an explicitly requested removal, deprecation decision or API migration. Account for Rust exports, Python aliases/kwargs, stubs/helpers, examples, docs, tests and known downstream consumers. Unknown external users of a published SDK are not zero consumers: preserve supported aliases/adapters until compatibility obligations are accounted for, or report the unresolved removal decision.
- Inactivity or legacy naming alone does not authorize deletion. Pattern examples do not authorize new flags, dependencies/frameworks, production traffic simulation, deployment/hardware checks or commits. Document compatibility window, observed evidence, rollback and unverified external/physical limits using the existing verification contract.
