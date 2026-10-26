# unitree-webrtc-rs

Rust-owned WebRTC transport layer for **Unitree Go2** robots, with a public native Rust session API and optional Python bindings via [PyO3](https://pyo3.rs) + [maturin](https://www.maturin.rs). Offline software checks cover macOS ARM64 and one Jetson aarch64 environment; bounded Go2 Air LocalSTA observations are summarized under [validation scope](#validation-scope). Other models and general Linux/Jetson compatibility are not established.

## Features

| Feature | Description |
|---|---|
| **Connection** | LocalAP / LocalSTA signaling with validated reconnect; Remote is not implemented |
| **DataChannel** | Validation, heartbeat, pub/sub, request/response over a single datachannel |
| **Sport Mode** | Send sport commands (`StandUp`, `Hello`, `FrontFlip`, …) and read state |
| **State Subscriptions** | Subscribe to topics (`SportModeState`, `LowState`, `MultipleState`, …) with callbacks |
| **LiDAR** | Native Rust decoder — LZ4 decompress → bit-unpack → NumPy `ndarray` (owned Rust buffer → NumPy without copying) |
| **Video Receive** | H.264 decode via GStreamer → NumPy BGR frames |
| **Audio Receive** | Opus decode → NumPy int16 samples (48 kHz stereo receive; decoder preserves configured metadata) |
| **Audio Transmit** | Play local files or stream URLs to the robot via GStreamer → Opus → RTP |

See [transport contracts and resource policy](docs/engineering/resource-policy.md) for limits, buffer ownership, test setup and Rust API migration.

## Architecture

See the [current architecture and extension map](docs/architecture_analysis.md) for composition, ownership, language boundaries and evidence-gated expansion seams.

```
┌─────────────────────────────────────────────────────────┐
│  Python (thin bridge)                                   │
│  UnitreeWebRTCConnection / DataChannelBridge / …        │
│  ──── PyO3 boundary ────────────────────────────────────│
│  Rust                                                   │
│  ┌──────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │Signaling │→ │ RTC Engine   │→ │DataChannel Svc   │  │
│  │(HTTP)    │  │ (webrtc-rs)  │  │(pub/sub/req/res) │  │
│  └──────────┘  └──────┬───────┘  └────────┬─────────┘  │
│                       │                    │            │
│              ┌────────┴────────┐   ┌───────┴────────┐  │
│              │ Media Tracks    │   │ LiDAR Worker   │  │
│              │ (video / audio) │   │ Pool           │  │
│              └────────┬────────┘   └───────┬────────┘  │
│                       │                    │            │
│              ── crossbeam channel ──────────────────    │
│                       │                    │            │
│              ┌────────┴────────────────────┴────────┐  │
│              │  Python Dispatcher Thread (GIL here)  │  │
│              │  → callbacks to user Python code      │  │
│              └───────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

**Key design decisions:**

- **Single-session ownership** — Rust owns the WebRTC peer connection lifecycle; Python never touches `aiortc`.
- **Rust-owned hot path** — WebRTC, LiDAR decode and media workers do not run Python code. API conversions and Python callbacks still require PyO3 attachment; the extension explicitly requires the GIL and does not claim free-threaded support.
- **Bounded resource admission** — count/byte budgets limit queued callback buffers; admission uses synchronization, and overflow has explicit error/drop policies. This is not a lock-free or whole-process memory guarantee.
- **Async integration** — `pyo3-async-runtimes` bridges Rust Tokio futures to Python `asyncio`; input conversion and Python callbacks still require Python attachment.

## Dependencies

### Rust (Cargo)

| Crate | Purpose |
|---|---|
| `pyo3` 0.28 | Python ↔ Rust bindings |
| `pyo3-async-runtimes` 0.28 | tokio future → Python asyncio |
| `numpy` 0.28 | Zero-copy NumPy array creation |
| `webrtc` 0.17.1 | Pure-Rust WebRTC stack |
| `tokio` 1.x | Async runtime |
| `crossbeam-channel` 0.5 | Bounded thread-to-thread delivery |
| `serde` / `serde_json` | JSON serialization for DataChannel messages |
| `lz4_flex` 0.11 | LZ4 decompression for LiDAR |
| `gstreamer` / `gstreamer-app` / `gstreamer-audio` / `gstreamer-video` 0.25 | Media decode (video H.264, audio Opus) and audio TX pipeline |
| `opus` 0.3 | Opus codec bindings (audio RX fallback) |
| `aes-gcm` / `rsa` / `md-5` | Encryption and token security for signaling |
| `reqwest` 0.13 | HTTP client for signaling |
| `tracing` / `tracing-subscriber` | Structured logging |

## Installation

Install a compatible published wheel from PyPI when available. The release workflow targets Ubuntu x86_64 and macOS Apple Silicon with Python 3.10, 3.11 and 3.12; it does not build Jetson aarch64 wheels. Jetson requires a source build unless a matching wheel is supplied separately.

```bash
pip install unitree-webrtc-rs
```

### System Dependencies

While the native WebRTC and LiDAR decoders are bundled, **GStreamer is strictly required** by the system to run Video decoding and Audio encoding/decoding.

#### Ubuntu / Jetson (JetPack 6 / Ubuntu 22.04+)
```bash
sudo apt-get update
sudo apt-get install -y \
    libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev \
    libopus-dev \
    gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad \
    gstreamer1.0-libav \
    pkg-config
```

#### macOS
```bash
brew install gstreamer opus pkg-config
```

### Optional Python Dependencies

For running the examples (Video viewing, Audio playing):
| Package | Purpose |
|---|---|
| `opencv-python >=4.8` | Optional — video display in examples |
| `pyaudio` | Optional — audio playback in examples |

## Development Setup

Contributor conventions, shared plugin routing and reusable task prompts: [CONTRIBUTING.md](CONTRIBUTING.md), [AGENTS.md](AGENTS.md), and [agent workflow](docs/engineering/agent-workflow.md).

### One-command bootstrap (recommended)

```bash
# from repository root
make bootstrap
```

`make bootstrap` runs all of the following:

- `make doctor` — checks required local tools and system libraries (`uv`, `cargo`, `pkg-config`, `glib-2.0`, `gobject-2.0`, `gstreamer-1.0`)
- `make init` — locked development dependency sync, pre-commit hook installation and native extension build
- `make acceptance` — guidance/format/lint/checks, Python-free and Python-linked Rust tests, then a fresh native build and Python tests

### Individual commands

```bash
make doctor     # prerequisite checks
make init       # uv sync + maturin develop
make build      # rebuild python extension
make check      # cargo fmt/clippy/check
make test       # Rust tests + fresh native build + pytest
```

### Build Verification (Rust side)

```bash
make check
```

> Rust tests link Python normally; only extension builds enable `extension-module`. Run `make test-rust` in the synced development environment. `make test-python` rebuilds the extension before pytest, including under `make -j`; mocks alone do not validate the native boundary. Cargo checks/tests and extension builds are serialized to avoid shared PyO3 linker-mode races. `make acceptance` runs guidance/checks, core, Rust and fresh-native tests; `make wheel-smoke WHEEL=/absolute/path/package.whl` installs and imports a wheel in a temporary environment outside the repo. Native GStreamer/Opus system libraries remain prerequisites on each platform. These are offline software gates, not robot, stress or platform-portability proof.

### Rust consumers without Python

The public `connection::UnitreeSession` owns validated lifecycle, queues, workers and audio policy; Python uses the same owner. Other native bindings can consume its owned `CallbackEvent` receiver instead of rebuilding these policies.

```toml
# In a consuming project's Cargo.toml; replace the local path.
unitree-webrtc-rs = { path = "/path/to/unitree-webrtc-rs", default-features = false }
```

```rust,ignore
use unitree_webrtc_rs::connection::{SessionOptions, UnitreeSession};
use unitree_webrtc_rs::protocol::constants::WebRTCConnectionMethod;

let options = SessionOptions::new(WebRTCConnectionMethod::LocalSTA, Some(robot_ip));
let (session, events) = UnitreeSession::new(options, aes_128_key)?;
// Consume the bounded owned-event receiver on your own worker, not a Tokio blocking wait.
session.connect().await?; // Real robot I/O: do not execute as an offline smoke test.
session.disconnect().await;
```

Run `make test-core` to check/test the Python-free dependency graph. Native GStreamer/Opus dependencies remain required; this is not a browser/WASM backend. Defaults retain Python compatibility; disabling default features is explicit. See [session ownership, options and Rust adapter migration](docs/engineering/resource-policy.md).

## Quick Start

### Connect & Disconnect

```python
import asyncio
from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod

async def main():
    conn = UnitreeWebRTCConnection(
        WebRTCConnectionMethod.LocalSTA,
        ip="192.168.12.1",
        # Required for Go2 firmware 1.1.15+ LAN handshakes that return data2=3.
        # If omitted, GO2_DORA_UNITREE_AES_128_KEY is checked.
        aes_128_key="00112233445566778899aabbccddeeff",
    )
    await conn.connect()
    print(f"Connected: {conn.is_connected}")

    await conn.disconnect()

asyncio.run(main())
```

For Go2 firmware `1.1.15+`, the LAN `/con_notify` handshake may use `data2=3`,
which requires the robot's per-device AES-128 hex key. Pass it with
`aes_128_key=...` or set `GO2_DORA_UNITREE_AES_128_KEY`.

`ip` is a host, not a URL or `host:port`; local signaling disables proxies and redirects. Construction can raise `RuntimeError` if a native worker/dispatcher cannot start. Malformed or oversized Python/peer inputs now fail at explicit software admission boundaries; these ceilings are not firmware maxima. See [resource and migration contracts](docs/engineering/resource-policy.md).

Disconnect retains callbacks for reconnect. For final cleanup, explicitly release callback captures, then await transport cleanup while the runtime is alive:

```python
conn.clear_callbacks()  # Synchronous; not disconnect/unsubscribe/media-off.
await conn.disconnect()
```

An already-admitted callback may finish; finalizers may register new callbacks. This is not terminal close or automatic garbage collection. See [callback ownership](docs/engineering/resource-policy.md#explicit-python-callback-release). Connection `connect`/`disconnect`/`reconnect` and audio playback/stop futures retain their existing `()` success value, reflected in the shipped [API stub](unitree_webrtc_rs.pyi).

### Query Sport State

```python
import asyncio
from unitree_webrtc_rs import (
    UnitreeWebRTCConnection, WebRTCConnectionMethod,
    RTC_TOPIC, SPORT_CMD,
)

async def main():
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="192.168.12.1")
    await conn.connect()

    # Query current state
    resp = await conn.datachannel.pub_sub.publish_request_new(
        RTC_TOPIC["SPORT_MOD"],
        {"api_id": SPORT_CMD["GetState"]},
    )
    print(resp)

    await conn.disconnect()

asyncio.run(main())
```

### Subscribe to State

```python
import asyncio
from unitree_webrtc_rs import (
    UnitreeWebRTCConnection, WebRTCConnectionMethod, RTC_TOPIC,
)

async def main():
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="192.168.12.1")
    await conn.connect()

    last_mode = None

    def on_state(msg):
        nonlocal last_mode
        last_mode = msg["data"]["mode"]

    conn.datachannel.pub_sub.subscribe(RTC_TOPIC["LF_SPORT_MOD_STATE"], on_state)
    await asyncio.sleep(10)
    await conn.disconnect()
    print(f"Last mode: {last_mode}")

asyncio.run(main())
```

### LiDAR Stream

```python
import asyncio
from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod

async def main():
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="192.168.12.1")
    await conn.connect()
    await conn.datachannel.disableTrafficSaving(True)
    conn.datachannel.set_decoder(decoder_type="native")
    conn.datachannel.pub_sub.publish_without_callback("rt/utlidar/switch", "on")

    received = 0

    def on_lidar(msg):
        nonlocal received
        received += 1
        points = msg["data"]["data"]["points"]   # numpy ndarray (N, 3) float32
        # Process points here; avoid printing on every callback.

    conn.datachannel.pub_sub.subscribe("rt/utlidar/voxel_map_compressed", on_lidar)
    await asyncio.sleep(10)
    await conn.disconnect()
    print(f"LiDAR frames received: {received}")

asyncio.run(main())
```

### Video Receive

```python
import asyncio
from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod

async def main():
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="192.168.12.1")
    await conn.connect()

    received = 0

    def on_frame(frame):
        nonlocal received
        received += 1
        # frame: numpy ndarray (H, W, 3) uint8 BGR
        # Process frame here; avoid printing on every callback.

    conn.video.on_frame(on_frame)
    conn.video.switchVideoChannel(True)

    await asyncio.sleep(10)

    conn.video.switchVideoChannel(False)
    await conn.disconnect()
    print(f"Video frames received: {received}")

asyncio.run(main())
```

### Audio Receive

Only on an authorized, verified audio-capable model:

```bash
uv run --locked --no-sync python examples/audio_receive.py --ip "$ROBOT_IP" --duration 5
```

The example validates native interleaved `int16` arrays, retains at most four frames (at most 96,000 samples each), and drops newest on overflow. PyAudio is optional and imported only when running reception; without it, the example analyzes samples without local playback. Blocking writes are offloaded and observed before closing the stream. Media disable, device cleanup and disconnect are attempted independently; a hung device operation can still delay cleanup. These are example-local limits, not codec/robot maxima or physical audio validation.

### Audio Transmit (PC → Robot)

Requires GStreamer and operator authorization: this plays sound on the robot. Supports local files (WAV/MP3/OGG) and HTTP stream URLs.

```bash
uv run --locked --no-sync python examples/audio_play_file.py --ip "$ROBOT_IP" --file /path/to/audio.wav --duration 5
uv run --locked --no-sync python examples/audio_stream_url.py --ip "$ROBOT_IP" --url https://example.com/stream.mp3 --duration 5
```

Both use shared address/auth/duration validation and attempt stop before independent disconnect, including after startup failure. Awaiting session audio stop observes owned native/RTP cleanup; canceling its waiter does not abandon that cleanup. This does not make a hung native call bounded, and final object drop is only best effort. Audio has not been independently physically validated.

## Python API Reference

### Module: `unitree_webrtc_rs`

All symbols are also available under `unitree_webrtc_rs.webrtc_driver` for import compatibility.

#### Classes

| Class | Description |
|---|---|
| `UnitreeWebRTCConnection` | Main connection object. Provides `.datachannel`, `.video`, `.audio` bridges. |
| `WebRTCConnectionMethod` | Enum-like: `LocalAP=1`, `LocalSTA=2`, `Remote=3` |
| `VUI_COLOR` | Color constants: `WHITE`, `RED`, `YELLOW`, `BLUE`, `GREEN`, `CYAN`, `PURPLE` |

#### Constants (dict)

| Name | Type | Description |
|---|---|---|
| `DATA_CHANNEL_TYPE` | `dict[str, str]` | DataChannel message types (`VALIDATION`, `SUBSCRIBE`, `MSG`, `REQUEST`, …) |
| `RTC_TOPIC` | `dict[str, str]` | Robot topic paths (`SPORT_MOD`, `LOW_STATE`, `ULIDAR`, …) |
| `SPORT_CMD` | `dict[str, int]` | Sport command IDs (`StandUp=1004`, `Hello=1016`, `GetState=1034`, …) |
| `AUDIO_API` | `dict[str, int]` | Audio hub API IDs (`GET_AUDIO_LIST=1001`, `ENTER_MEGAPHONE=4001`, …) |
| `APP_ERROR_MESSAGES` | `dict[str, str]` | Error code to human-readable message mapping |

```python
# Import examples
from unitree_webrtc_rs import RTC_TOPIC, SPORT_CMD, AUDIO_API, VUI_COLOR
from unitree_webrtc_rs.webrtc_driver import RTC_TOPIC, SPORT_CMD  # also works
```

## Project Structure

```text
unitree-webrtc-rs/
├── Cargo.toml               # Rust dependencies
├── pyproject.toml           # Python project / uv build config
├── unitree_webrtc_rs.pyi    # Shipped native API typing
├── AGENTS.md               # Agent working agreement
├── .agents/               # Shared-plugin routing/setup and retained legal notices
├── src/
│   ├── audio/               # Audio streaming (GStreamer send/receive)
│   ├── connection/          # session.rs owner, lifecycle.rs procedures, service.rs transport
│   ├── datachannel/         # Data routing, pub/sub, and LiDAR codec
│   ├── infrastructure/      # Low-level WebRTC engine / HTTP signaling
│   ├── interface/           # PyO3 bridges, Python-facing APIs, and callbacks
│   ├── protocol/            # Models, ports, and WebRTC constants
│   ├── video/               # H.264 video RTP streaming & GStreamer
│   └── lib.rs               # PyO3 module entry point
├── tests/
│   ├── example_flows_test.rs # Rust integration tests (no real robot needed)
│   ├── test_python_api.py   # Python API mocked pytest suite
│   ├── test_native_contracts.py # Actual-extension offline contracts
│   ├── test_example_safety.py # Fake-transport example safety
│   └── test_version_sync.py # Release tooling and actual offline Cargo fixture
└── examples/
    ├── example_support.py  # Example-only CLI/auth/cleanup policy
    ├── connect.py
    ├── sport_mode.py
    ├── sport_mode_state.py
    ├── lidar_stream.py
    ├── video_stream.py
    ├── audio_receive.py
    ├── audio_play_file.py
    └── audio_stream_url.py
```

## GStreamer Dependency

GStreamer is required for **video receive** (H.264 decode) and **audio transmit** (encode + RTP).

### macOS

```bash
brew install gstreamer
```

### Ubuntu / Jetson

```bash
sudo apt install -y \
    libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev \
    libopus-dev \
    gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad \
    gstreamer1.0-libav
```

If audio transmit fails, ensure `opusenc` and `rtpopuspay` plugins are available:

```bash
gst-inspect-1.0 opusenc
gst-inspect-1.0 rtpopuspay
```

### Audio TX GStreamer Pipeline (internal)

```
uridecodebin → audioconvert → audioresample → opusenc → rtpopuspay → appsink
                                                                        ↓
                                                         webrtc-rs TrackLocalStaticRTP
```

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `UNITREE_WEBRTC_RS_LOG` | `warn` | Rust tracing target/level filter; set before first import |
| `UNITREE_WEBRTC_RS_LOG_INIT` | `1` | `0` leaves subscriber installation to the host; `1` enables SDK fallback initialization |
| `ROBOT_IP` | — | Used by examples as fallback IP |

### Logging from Python

The SDK reuses **tracing + tracing-subscriber**, not a separate logger framework. The SDK-owned fallback writes WARN/ERROR to **stderr**; importing the module has no default banner. Configure before first import:

```bash
# Routine use
UNITREE_WEBRTC_RS_LOG=warn python your_program.py
# SDK lifecycle information without dependency debug noise
UNITREE_WEBRTC_RS_LOG='warn,unitree_webrtc_rs=info' python your_program.py
# Investigate only the connection boundary
UNITREE_WEBRTC_RS_LOG='warn,unitree_webrtc_rs::connection=debug' python your_program.py
```

An existing Rust subscriber owns its filter/writer; the SDK does not replace it. Set `UNITREE_WEBRTC_RS_LOG_INIT=0` if the host will install a subscriber later. Invalid explicit SDK settings raise `ValueError` when SDK initialization is enabled; disabled initialization and existing host ownership ignore unused SDK settings. Changing environment variables after import does not reload the logger.

Repeated queue/decode/callback warnings are owner-local summaries: first occurrence immediately, then a five-second window with counts. Flush is event-driven or best-effort on owner teardown, not a new five-second timer. Terminal stream failures remain ERROR and stop the failed stream. Python callback diagnostics use numeric registration IDs and fixed builtin exception classes, not secret-bearing exception text or raw topics.

Rust filters do not control Python `logging`/`print` or native GStreamer `GST_DEBUG`. File rotation/retention belongs to the host/collector; adding a Python rotating handler alone does not capture native stderr. These are selected SDK repeat limits, not a whole-process log/RSS cap. See [logging and acceptance policy](docs/engineering/conventions.md#8-logging-and-acceptance-policy).

## Examples

Run after `make build`. The examples below and the three [audio examples](#audio-receive) require `--ip` or `ROBOT_IP`; no device address is selected implicitly. Supply the per-device key with `UNITREE_AES_128_KEY`, a private `UNITREE_AES_128_KEY_FILE`, or `--aes-128-key-file -` (stdin). Do not put real keys in shell history or command arguments. `--duration` is finite and bounded. These examples exit nonzero on failed connection, missing/invalid data or incomplete cleanup.

```bash
# Set ROBOT_IP to your device, never a repository-wide default.
uv run --locked --no-sync python examples/connect.py --ip "$ROBOT_IP" --duration 3

# Read-only query and state subscription
uv run --locked --no-sync python examples/sport_mode.py --ip "$ROBOT_IP" --duration 3
uv run --locked --no-sync python examples/lowstate_test.py --ip "$ROBOT_IP" --duration 3
uv run --locked --no-sync python examples/sport_mode_state.py --ip "$ROBOT_IP" --duration 3

# Only with operator authorization, clear surroundings and a lowered starting posture:
# StandUp -> StandDown; no mode switching or additional motion catalog.
uv run --locked --no-sync python examples/sport_mode.py --ip "$ROBOT_IP" --allow-motion --duration 6

# StandUp only: intentionally leaves the robot standing for sensor tests.
uv run --locked --no-sync python examples/sport_mode.py --ip "$ROBOT_IP" --allow-motion --posture stand-up --duration 3

# StandDown only: requires a standing starting posture.
uv run --locked --no-sync python examples/sport_mode.py --ip "$ROBOT_IP" --allow-motion --posture stand-down --duration 3

# LiDAR point cloud stream (sensor remains on)
uv run --locked --no-sync python examples/lidar_stream.py --ip "$ROBOT_IP" --duration 5
uv run --locked --no-sync python examples/lidar_test.py --ip "$ROBOT_IP" --duration 5

# Headless video capture (requires opencv-python for image output)
uv run --locked --no-sync python examples/video_stream.py --ip "$ROBOT_IP" --headless --duration 8 --output /tmp/unitree-camera.png

# Non-audio integration; action_validation defaults to read-only GetState
uv run --locked --no-sync python examples/integration.py --ip "$ROBOT_IP" --duration 5
uv run --locked --no-sync python examples/action_validation.py --ip "$ROBOT_IP" --duration 3
```

`action_validation.py` reuses the safe sport flow; `lidar_test.py` reuses native LiDAR validation. Their duplicated implementations were removed, not their runnable entry points. The unsafe `--full-sequence` and old mode/VUI/multi-control options were removed intentionally; unknown flags fail rather than silently selecting a different motion sequence. SDK imports, aliases, topic names and wire IDs are unchanged.

The posture flow accepts only `normal` or `mcf`, preserving fresh orientation/heartbeat and observed height-change checks. Only StandUp/StandDown are verified to share IDs across these modes; other sport commands are not implicitly MCF-compatible. It never selects a different motion mode. `--allow-motion` without `--posture` runs the default up/down cycle.

Example failures distinguish `reason=unsupported_posture_mode` and `reason=lidar_stream_missing_invalid_or_stale` without printing peer/exception text. A failed LiDAR validation triggers example-owned cleanup/disconnect; that log alone does not prove the transport broke. See [validation scope](#validation-scope).

Audio examples remain separate and were not run on Go2 Air. Integration audio is **opt-in** via `--include-audio` only on a verified audio-capable model. These example changes do not establish support for another robot or implement a capability-negotiation layer.

## Validation scope

Offline software checks cover macOS ARM64 and one Jetson aarch64 environment (Ubuntu 22.04.5, Python 3.12.12, GStreamer 1.20.x); the Jetson native build/import and contract/logging checks passed. In a bounded observation on that Jetson and one Go2, a single StandUp produced a measured height change and user-confirmed normal standing, followed by valid native LiDAR callbacks with healthy heartbeat/RPCs; an earlier lowered-posture window had no LiDAR callbacks but retained a healthy transport. These observations preceded final release cleanup, are not benchmarks, and do not certify portable wheels or all JetPack versions.

Intermittent ICE-checking timeouts remain unresolved and occurred before LiDAR was enabled. Audio and StandDown have not been independently physically verified by the agent; stress/fleet behavior, repeated posture transitions, free-threaded Python and other robot models remain unverified.

## License

Proprietary — Battlebang / jongmoon.choi
