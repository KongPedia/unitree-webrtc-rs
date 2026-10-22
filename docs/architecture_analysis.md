# Architecture and extension context

This map describes the Rust-owned SDK, not a proposed Python package tree. Use it to locate responsibilities and assess extensions; exact runtime limits and migration details belong in [resource-policy.md](engineering/resource-policy.md), and design rules belong in [conventions.md](engineering/conventions.md).

## Composition and dependencies

```text
Python application                         Native Rust consumer
       |                                           |
interface/bridges + callbacks                       |
       +-------------------+-----------------------+
                           v
             connection::UnitreeSession
             session.rs: composition/resource owner
             lifecycle.rs: validated connect/reset/rollback
                           |
           +---------------+----------------+
           v                                v
  ConnectionService                 DataChannelService
  transport establishment           framing/router/chunks/RPC/heartbeat
           |                                |
           +------ protocol/ports ----------+
                      ^ implementations
                      |
        infrastructure: HTTP signaling + RTC engine
                      |
       WebRTC datachannel and audio/video tracks
```

- [connection/session.rs](../src/connection/session.rs) assembles signaling/RTC adapters, ingress and callback channels, LiDAR workers and audio playback ownership. `UnitreeSession::new` selects concrete adapters; `with_adapters` accepts offline/custom ports. Both return the session and its owned event receiver.
- [protocol/](../src/protocol/) defines ports, constants, wire/ingress models and callback events without PyO3, GStreamer or HTTP implementation dependencies. [infrastructure/](../src/infrastructure/) implements signaling/security, RTC setup/handlers, outbound sending and tracked peer cleanup. This is not a claim that every module is framework-independent: core validation reuses the security helper, and media handlers/codecs use WebRTC/GStreamer types.
- [datachannel/core/](../src/datachannel/core/) owns routing, validation, heartbeat, subscriptions and request completion. `framing.rs` parses the existing binary layouts; `chunks.rs` owns bounded chunk assembly. These are not application-specific sport/SLAM services.
- [datachannel/lidar/](../src/datachannel/lidar/) owns compressed-job scheduling and native decoding. [video/](../src/video/) owns H.264 assembly/decode; [audio/receive/](../src/audio/receive/) owns Opus receive and [audio/transmit/](../src/audio/transmit/) owns playback/encoding/RTP production.
- [interface/](../src/interface/) marshals the Python API, aliases, inputs/errors and callbacks. It delegates session policy to Rust instead of maintaining another transport implementation.

## Dataflow, ownership and lifecycle

RTC ingress carries an epoch captured when the peer handler is created. The core rejects stale ingress under the same synchronization boundary as reset. Topic messages become callback events; compressed LiDAR jobs pass through the worker pool; audio/video tracks pass through their codecs. All callback producers retain the epoch of the work they received, rather than relabeling old work with the current session.

[protocol/events.rs](../src/protocol/events.rs) admits owned callback buffers into separate control/media count and byte quotas. The Python dispatcher retains the admission permit through attachment/conversion/callback handling and rechecks the epoch before callback admission. Native receiver methods transfer event ownership and release the permit; user-retained buffers/NumPy arrays are outside the quota. Admission uses synchronization, not a lock-free guarantee. Control-first callback selection does not guarantee control delivery through the shared ingress queue.

The RTC adapter owns a bounded outbound command worker. Successful enqueue means local acceptance, not wire completion or a robot acknowledgement; a send already in progress cannot be recalled. Pending request admission, response correlation, cancellation and reset belong to the core. Exact bounds, overflow behavior and sticky failure paths are documented in [resource policy](engineering/resource-policy.md#named-safety-policies).

Connect/reconnect/disconnect and concrete audio playback share the Rust lifecycle owner. UnitreeSession readiness requires peer validation as well as transport establishment; the low-level `ConnectionService::is_connected` reports completion of transport establishment, not live validated readiness. Disconnect invalidates active/queued connection attempts, fails pending requests, clears retained chunk assembly and invalidates stale callback/decode work by epoch. It retains reusable queues/workers and the event receiver for reconnect. Await disconnect while the Tokio runtime remains alive: final owner drop stops core work and schedules best-effort asynchronous transport cleanup, not a guaranteed async join. Python children retain the complete session owner; a raw Rust datachannel handle alone does not own a validated session. An already-admitted Python callback may finish after reset.

## Language and buffer boundary

Python is an optional default feature; extension builds enable `extension-module`, while `--no-default-features` excludes PyO3/NumPy and Python build helpers. Native consumers still require the GStreamer/Opus system dependencies; this is not a browser/WASM backend.

Rust owns network parsing, codecs, worker queues and media buffers. Python access/conversion and callbacks attach to Python; callback-registry locks are released before user code runs. NumPy receives owned LiDAR `float32 (N, 3)`, BGR video `uint8 (H, W, 3)` and interleaved audio `int16 (samples,)` arrays. The owned Rust buffer → NumPy handoff avoids copying at that boundary; parsing, decompression, decoding and stride repacking may still allocate/copy. No end-to-end zero-copy or free-threaded Python safety claim follows.

## Migrating to 0.3

Python imports, legacy aliases, valid wire fields/command IDs and callback values remain compatible; malformed/ambiguous inputs now fail explicitly. The shipped Python typing source is the root [unitree_webrtc_rs.pyi](../unitree_webrtc_rs.pyi); unused duplicate Python helpers/stubs were removed. Example CLI changes are intentional and documented in [README examples](../README.md#examples).

Rust source consumers must update ingress adapters for `IngressMessage` and fallible epoch setup, use the event sender/receiver wrappers instead of raw callback channels, handle local `TransportFault`/readiness variants in exhaustive matches, and handle the fallible LiDAR point decoder. Prefer public `UnitreeSession` for shared validated lifecycle/resource ownership. Consult the [complete migration and buffer contracts](engineering/resource-policy.md#compatibility-and-buffer-contracts) before updating external adapters; wrappers do not emulate every crossbeam API.

## Practical extension seams

| Change | Existing seam and evidence required |
| --- | --- |
| New topic or command helper | Reuse core pub/sub/request operations and canonical protocol constants. Establish the actual topic/schema/API ID and sanitized positive/error fixtures; do not duplicate framing or session policy. |
| Another firmware or robot model | Compare signaling/auth, topics, codecs and capabilities against sourced firmware/model evidence. Keep proven differences at adapter/composition boundaries; reject unknown capabilities rather than silently sending Go2 commands. Upstream Python support alone is not Rust or hardware support. |
| Another native language binding | Consume `UnitreeSession` and owned events; add only language-specific conversion, lifetime and typing contracts. No Node binding or universal wrapper package is currently implemented. |
| Media or performance change | Extend the owning codec/worker, retain input/resource bounds and terminal error behavior, and add focused fixtures. Measure latency, retained memory and drops before claiming an improvement or changing budgets. |

Do not create speculative sport/state/SLAM/UWB/arm packages or a profile/plugin registry just because a topic exists. A second verified implementation or actual caller need should drive that boundary. Multi-model support and fleet scale are separate problems: per-session safety ceilings do not establish whole-process memory or multi-robot latency guarantees.

## Verification boundaries

Use [CONTRIBUTING](../CONTRIBUTING.md#setup-and-verification) for applicable regression/native/packaging checks and [README validation scope](../README.md#validation-scope) for bounded hardware observations and unresolved ICE behavior. A build, mock test, queue admission or RPC ACK does not establish physical success. Remote signaling, broad platform/model support, long-running fleet/stress behavior and independent audio validation remain outside the demonstrated scope.
