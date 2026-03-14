# unitree-webrtc-rs

Rust-owned WebRTC transport layer for **Unitree Go2** robots, exposed to Python via [PyO3](https://pyo3.rs) + [maturin](https://www.maturin.rs). Built for Jetson (ARM64) performance while remaining fully usable on x86 dev machines.

## Features

| Feature | Description |
|---|---|
| **Connection** | WebRTC signaling (LocalAP / LocalSTA / Remote) with auto-reconnect |
| **DataChannel** | Validation, heartbeat, pub/sub, request/response over a single datachannel |
| **Sport Mode** | Send sport commands (`StandUp`, `Hello`, `FrontFlip`, …) and read state |
| **State Subscriptions** | Subscribe to topics (`SportModeState`, `LowState`, `MultipleState`, …) with callbacks |
| **LiDAR** | Native Rust decoder — LZ4 decompress → bit-unpack → NumPy `ndarray` (zero-copy) |
| **Video Receive** | H.264 decode via GStreamer → NumPy BGR frames (~14 FPS) |
| **Audio Receive** | Opus decode → NumPy int16 samples (~50 FPS, 48 kHz stereo) |
| **Audio Transmit** | Play local files or stream URLs to the robot via GStreamer → Opus → RTP |

## Architecture

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
- **GIL-free hot path** — Only the dedicated dispatcher thread enters the GIL to invoke Python callbacks. WebRTC, LiDAR decode, and media threads remain free-threaded.
- **Lock-free data flow** — `crossbeam-channel` bounded queues for thread-to-thread delivery; atomics for shared flags.
- **Async integration** — `pyo3-async-runtimes` bridges Rust tokio futures to Python `asyncio` natively, releasing the GIL immediately.

## Dependencies

### Rust (Cargo)

| Crate | Purpose |
|---|---|
| `pyo3` 0.28 | Python ↔ Rust bindings |
| `pyo3-async-runtimes` 0.28 | tokio future → Python asyncio |
| `numpy` 0.28 | Zero-copy NumPy array creation |
| `webrtc` 0.17.1 | Pure-Rust WebRTC stack |
| `tokio` 1.x | Async runtime |
| `crossbeam-channel` 0.5 | Lock-free bounded channels |
| `serde` / `serde_json` | JSON serialization for DataChannel messages |
| `lz4_flex` 0.11 | LZ4 decompression for LiDAR |
| `gstreamer` / `gstreamer-app` / `gstreamer-audio` / `gstreamer-video` 0.25 | Media decode (video H.264, audio Opus) and audio TX pipeline |
| `opus` 0.3 | Opus codec bindings (audio RX fallback) |
| `aes-gcm` / `rsa` / `md-5` | Encryption and token security for signaling |
| `reqwest` 0.13 | HTTP client for signaling |
| `tracing` / `tracing-subscriber` | Structured logging |

### System (must be installed)

| Dependency | Install |
|---|---|
| **GStreamer 1.20+** | `brew install gstreamer` (macOS) / `apt install libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav` (Ubuntu/Jetson) |
| **pkg-config** | `brew install pkg-config` / `apt install pkg-config` |
| **OpenSSL** | Usually pre-installed; `apt install libssl-dev` if missing |
| **Rust toolchain** | [rustup.rs](https://rustup.rs) (1.75+) |

### Python

| Package | Purpose |
|---|---|
| `numpy >=1.24` | Required — frame/point array types |
| `maturin >=1.5` | Build tool (dev only) |
| `opencv-python >=4.8` | Optional — video display in examples |
| `pyaudio` | Optional — audio playback in examples |

## Development Setup

```bash
# 1. Navigate to this directory
cd src/third_party/unitree-webrtc-rs

# 2. Create & activate virtual environment (uv recommended)
uv venv
source .venv/bin/activate

# 3. Install Python dependencies
uv pip install -e ".[dev]"

# 4. Build the Rust extension into the venv
maturin develop

# 5. Verify the import
python -c "from unitree_webrtc_rs import UnitreeWebRTCConnection, RTC_TOPIC, SPORT_CMD; print('OK')"
```

### Build Verification (Rust side)

```bash
cargo fmt                                                    # format
cargo clippy --all-targets --all-features -- -D warnings     # lint (0 warnings required)
cargo check --tests                                          # type-check tests
```

> **Note:** `cargo test` may fail on macOS ARM64 due to PyO3 linker issues. Use `cargo check --tests` as a substitute. Full test suite runs on Linux/Jetson.

## Quick Start

### Connect & Disconnect

```python
import asyncio
from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod

async def main():
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="192.168.12.1")
    await conn.connect()
    print(f"Connected: {conn.is_connected}")

    await conn.disconnect()

asyncio.run(main())
```

### Send Sport Command

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

    # Execute a movement
    await conn.datachannel.pub_sub.publish_request_new(
        RTC_TOPIC["SPORT_MOD"],
        {"api_id": SPORT_CMD["Hello"]},
    )

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

    def on_state(msg):
        print(msg["data"]["mode"])

    conn.datachannel.pub_sub.subscribe(RTC_TOPIC["LF_SPORT_MOD_STATE"], on_state)
    await asyncio.sleep(10)
    await conn.disconnect()

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

    def on_lidar(msg):
        points = msg["data"]["data"]["points"]   # numpy ndarray (N, 3) float64
        print(f"points: {points.shape}")

    conn.datachannel.pub_sub.subscribe("rt/utlidar/voxel_map_compressed", on_lidar)
    await asyncio.sleep(10)
    await conn.disconnect()

asyncio.run(main())
```

### Video Receive

```python
import asyncio
from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod

async def main():
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="192.168.12.1")
    await conn.connect()

    def on_frame(frame):
        # frame: numpy ndarray (H, W, 3) uint8 BGR
        print(f"frame: {frame.shape}")

    conn.video.on_frame(on_frame)
    conn.video.switchVideoChannel(True)

    await asyncio.sleep(10)

    conn.video.switchVideoChannel(False)
    await conn.disconnect()

asyncio.run(main())
```

### Audio Receive

```python
import asyncio
from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod

async def main():
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="192.168.12.1")
    await conn.connect()

    def on_audio(samples):
        # samples: numpy ndarray int16, 1920 samples per frame, 48kHz stereo
        print(f"audio: {samples.shape} dtype={samples.dtype}")

    conn.audio.on_audio(on_audio)
    conn.audio.switchAudioChannel(True)

    await asyncio.sleep(10)

    conn.audio.switchAudioChannel(False)
    await conn.disconnect()

asyncio.run(main())
```

### Audio Transmit (PC → Robot)

Requires GStreamer installed. Supports local files (WAV/MP3/OGG) and HTTP stream URLs.

```python
import asyncio
from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod

async def main():
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="192.168.12.1")
    await conn.connect()

    # Play a local file
    await conn.audio.play_from_file("/path/to/audio.wav")
    await asyncio.sleep(10)
    await conn.audio.stop()

    # Or stream from URL
    await conn.audio.play_from_url("https://example.com/stream.mp3")
    await asyncio.sleep(10)
    await conn.audio.stop()

    await conn.disconnect()

asyncio.run(main())
```

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

```
unitree-webrtc-rs/
├── Cargo.toml              # Rust dependencies
├── pyproject.toml           # Python project / maturin config
├── src/
│   ├── lib.rs               # PyO3 module entry point
│   ├── interface/
│   │   ├── py_api.rs        # Python-facing classes (Connection, Bridges)
│   │   └── constants.rs     # All protocol constants (topics, commands, errors)
│   ├── application/
│   │   ├── connection_service.rs
│   │   ├── datachannel_service.rs
│   │   ├── lidar_service.rs
│   │   ├── video_service.rs
│   │   ├── audio_service.rs
│   │   └── audio_sender_service.rs
│   ├── infrastructure/
│   │   ├── rtc_engine.rs     # WebRTC peer connection management
│   │   └── signaling_http.rs # HTTP signaling client
│   └── domain/
│       └── models.rs         # DcMessage, CallbackEvent, etc.
└── examples/
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
| `UNITREE_WEBRTC_RS_LOG` | `unitree_webrtc_rs=info,webrtc=error,...` | Tracing filter (see [EnvFilter](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)) |
| `ROBOT_IP` | — | Used by examples as fallback IP |

## Examples

All examples are in the `examples/` directory. Run after `maturin develop`:

```bash
# Basic connection test
python examples/connect.py --ip 10.2.80.114

# Sport mode commands (safe: GetState only)
python examples/sport_mode.py --ip 10.2.80.114

# Sport mode state subscription (live IMU/pose display)
python examples/sport_mode_state.py --ip 10.2.80.114

# LiDAR point cloud stream
python examples/lidar_stream.py --ip 10.2.80.114

# Video stream (requires opencv-python)
python examples/video_stream.py --ip 10.2.80.114

# Audio receive (requires pyaudio)
python examples/audio_receive.py --ip 10.2.80.114

# Audio transmit: local file
python examples/audio_play_file.py --ip 10.2.80.114 --file /path/to/audio.wav

# Audio transmit: URL stream
python examples/audio_stream_url.py --ip 10.2.80.114 --url https://example.com/stream.mp3
```

## Benchmarks

Tested on Go2 LocalSTA connection:

| Feature | Metric |
|---|---|
| **LiDAR** | ~7.8 FPS, ~28.8k points/frame, ~224k pts/s throughput |
| **Video** | ~14 FPS (H.264 → BGR NumPy) |
| **Audio RX** | ~50 FPS (Opus → int16 1920 samples) |
| **Audio TX** | 200-600+ RTP packets / 10s (file or URL) |

## License

Proprietary — Battlebang / jongmoon.choi
