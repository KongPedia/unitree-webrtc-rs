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
- Log actionable errors with topic/session and redacted context. A repeated recoverable callback error should be observable without killing transport; swallowing every error is not resilience.
- Heavy Rust work and blocking waits should not keep Python attached unnecessarily. Python object access still requires PyO3 attachment. `Python::detach` does not make shared data race-free.
- `unsafe impl Send/Sync` needs a `SAFETY` rationale covering actual library thread affinity and all calling paths. PyO3 0.28's free-threading default is an attestation to audit, not proof that custom unsafe code or GStreamer use is safe.
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
