# unitree-webrtc-rs 아키텍처 분석 및 구조 제안


```
unitree_webrtc/
├── connection/         # connect, disconnect, reconnect, auto-reconnect
├── datachannel/
│   ├── core/           # validation, heartbeat, message routing, (base pub/sub)
│   ├── sport/          # sport mode commands, GetState
│   ├── state/          # subscription wrappers (SportModeState, LowState, etc.)
│   ├── lidar/          # lidar switch, subscription, decoder selection
│   ├── slam/           # (향후) SLAM mapping, navigation
│   ├── uwb/            # (향후) UWB tracking
│   ├── vui/            # (향후) VUI
│   └── arm/            # (향후) arm control
├── video/              # video receive, H.264 decode
├── audio/
│   ├── receive/        # audio RX (Opus decode)
│   └── transmit/       # audio TX (file/URL → GStreamer → RTP)
└── protocol/           # RTC_TOPIC, SPORT_CMD, AUDIO_API, DATA_CHANNEL_TYPE constants
```

### 4.2 옵션 C 상세 설계 근거

#### `connection/` — 독립 모듈

Connection은 **모든 기능의 전제조건**이며, 독자적인 lifecycle을 가짐:
- `connect()` → signaling → SDP exchange → DataChannel open
- `disconnect()` → intentional close
- `reconnect()` → close + re-connect
- `auto_reconnect(max_retries)` → exponential backoff loop

```python
# connection/
#   __init__.py
#   connection.py      → UnitreeConnection  (connect, disconnect, reconnect, auto_reconnect)
#   signaling.py       → (내부) HTTP signaling wrapper
#   types.py           → ConnectionMethod enum, ConnectionState
```

#### `datachannel/core/` — 공용 메시징 인프라

DataChannel의 **validation, heartbeat, pub/sub 메커니즘**은 모든 datachannel 기능이 공유:

```python
# datachannel/core/
#   pub_sub.py          → PubSub  (subscribe, unsubscribe, publish, publish_request_new)
#   message_types.py    → DataChannelMessageType enum
```

#### `datachannel/sport/` — Sport Mode 명령

Sport Mode는 **특정 topic(`rt/api/sport/request`)에 특정 패턴(`api_id` + `parameter`)으로 publish**하는 것:

```python
# datachannel/sport/
#   commands.py         → SportCommand enum (StandUp=1004, Hello=1016, ...)
#   sport_client.py     → SportClient (send_command, get_state, move, euler, ...)
```

#### `datachannel/state/` vs `datachannel/lidar/` — 왜 분리?

| 비교 항목 | State | LiDAR |
|---|---|---|
| Transport | subscribe (text JSON) | subscribe (binary + LZ4) |
| 디코딩 | JSON parse only | LZ4 decompress → bit-unpack → NumPy |
| 스레드 | DC Router → Dispatcher | DC Router → **Worker Pool** → Dispatcher |
| 데이터 크기 | 수 KB | 수십~수백 KB (28.8k points/frame) |
| 스로틀링 | 없음 | 15 FPS 제한 |
| 디코더 선택 | 없음 | `native` vs `libvoxel` |

**결론: State와 LiDAR는 같은 subscribe 메커니즘을 사용하지만, 데이터 처리 파이프라인이 완전히 다르므로 분리가 맞음.**

```python
# datachannel/state/
#   subscriptions.py    → StateSubscriber (sport_mode_state, low_state, etc.)
#   models.py           → SportModeState, LowState 등 typed models

# datachannel/lidar/
#   lidar_client.py     → LidarClient (switch, subscribe, set_decoder)
#   models.py           → LidarFrame (points: ndarray, metadata)
```

#### `video/` & `audio/` — Media Track 기반 독립

Video와 Audio는 **WebRTC media track**으로 수신하며, DataChannel과는 완전 별개 경로:

```python
# video/
#   video_receiver.py   → VideoReceiver (on_frame, switch_channel)

# audio/
#   receive/
#     audio_receiver.py → AudioReceiver (on_audio, switch_channel)
#   transmit/
#     audio_sender.py   → AudioSender (play_from_file, play_from_url, stop)
```

Audio TX/RX를 분리하는 이유: RX는 track callback이고 TX는 GStreamer 파이프라인으로 완전히 다른 구현.

#### `protocol/` — 프로토콜 상수 집중

```python
# protocol/
#   topics.py           → RTC_TOPIC dict
#   sport_commands.py   → SPORT_CMD dict
#   audio_api.py        → AUDIO_API dict
#   channel_types.py    → DATA_CHANNEL_TYPE dict
#   errors.py           → APP_ERROR_MESSAGES dict
#   vui_colors.py       → VUI_COLOR
```

---

## 5. 확장성 설계

### 5.1 새 DataChannel 기능 추가 시

새로운 토픽 그룹 (예: SLAM)을 추가할 때:

```
datachannel/
├── core/                # ← 변경 없음
├── sport/               # ← 변경 없음
├── state/               # ← 변경 없음
├── lidar/               # ← 변경 없음
└── slam/                # ← 새 폴더만 추가
    ├── __init__.py
    ├── slam_client.py   # subscribe("rt/uslam/*"), send commands
    └── models.py        # SlamOdom, CloudPoint, GlobalPath 등
```

**영향 범위**: 새 폴더 하나 추가 + `protocol/topics.py`에 이미 정의된 상수 사용. 기존 코드 변경 없음.

### 5.2 미사용 토픽 → 모듈 매핑

| 미래 모듈 | 사용할 토픽 | 구현 패턴 |
|---|---|---|
| `datachannel/slam/` | `SLAM_*`, `LIDAR_MAPPING_*`, `LIDAR_LOCALIZATION_*`, `LIDAR_NAVIGATION_*` | subscribe + command publish |
| `datachannel/uwb/` | `UWB_REQ`, `UWB_STATE` | request + subscribe |
| `datachannel/vui/` | `VUI` | request (api_id pattern) |
| `datachannel/obstacles/` | `OBSTACLES_AVOID` | request |
| `datachannel/arm/` | `ARM_COMMAND`, `ARM_FEEDBACK` | publish + subscribe |
| `datachannel/gas/` | `GAS_SENSOR`, `GAS_SENSOR_REQ` | request + subscribe |
| `datachannel/grid_map/` | `GRID_MAP` | subscribe |
| `datachannel/bash/` | `BASH_REQ` | request |
| `datachannel/motion/` | `MOTION_SWITCHER` | request |

---

## 6. Rust → Python 매핑: 현재 vs 제안

### 현재 (단일 py_api.rs에 모든 것)

```python
conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip="...")
await conn.connect()
conn.datachannel.pub_sub.subscribe(topic, callback)     # 모든 subscribe가 같은 경로
conn.datachannel.pub_sub.publish_request_new(topic, {})  # 모든 publish가 같은 경로
conn.video.on_frame(callback)
conn.audio.on_audio(callback)
```

### 제안: Rust PyO3 클래스는 유지, Python wrapper로 상위 추상화

```python
# 현재 Rust API는 그대로 유지 (breaking change 없음)
# Python 측에서 feature-oriented wrapper 추가

from unitree_webrtc_rs.sport import SportClient
from unitree_webrtc_rs.state import StateSubscriber
from unitree_webrtc_rs.lidar import LidarClient

conn = UnitreeWebRTCConnection(...)
await conn.connect()

sport = SportClient(conn)           # conn.datachannel.pub_sub 을 내부적으로 사용
await sport.stand_up()
await sport.hello()
state = await sport.get_state()

subscriber = StateSubscriber(conn)  # subscribe wrapper
subscriber.on_sport_mode_state(callback)
subscriber.on_low_state(callback)

lidar = LidarClient(conn)           # lidar-specific wrapper
lidar.enable()
lidar.set_decoder("native")
lidar.on_points(callback)
```

> [!IMPORTANT]
> **핵심 원칙**: Rust PyO3 바인딩(현재 `py_api.rs`)은 변경하지 않음. Python thin wrapper 레이어를 추가하여 feature-oriented API를 제공. backward compatibility 100% 유지.

---

## 7. 요약: Rust 구조 vs Python 구조

| Rust (Internal) | Python (User-facing) | 이유 |
|---|---|---|
| `connection_service.rs` | `connection/` | 1:1 대응 |
| `datachannel_service.rs` (1153 lines) | `datachannel/core/` + `sport/` + `state/` + `lidar/` + ... | 단일 거대 서비스를 도메인별로 분할 |
| `video_service.rs` | `video/` | 1:1 대응 |
| `audio_service.rs` | `audio/receive/` | 1:1 대응 |
| `audio_sender_service.rs` | `audio/transmit/` | 1:1 대응 |
| `lidar_service.rs` + `lidar_codec.rs` | `datachannel/lidar/` | Rust에서는 infra+app 분리, Python에서는 기능 단위 |
| `constants.rs` | `protocol/` | 상수를 카테고리별 파일로 분리 |

---

## 8. 체계적 오픈소스 프로젝트를 위한 추가 권장사항

### 8.1 README 기능 매트릭스
현재 README의 Feature 테이블과 Project Structure를 제안된 구조로 업데이트하면, 프로젝트에 어떤 기능이 있는지 폴더 레벨로 즉시 파악 가능.

### 8.2 Examples 폴더 구조
```
examples/
├── connection/
│   └── connect.py
├── sport/
│   ├── sport_mode.py
│   └── sport_mode_state.py
├── lidar/
│   ├── lidar_stream.py
│   └── lidar_test.py
├── video/
│   └── video_stream.py
├── audio/
│   ├── audio_receive.py
│   ├── audio_play_file.py
│   └── audio_stream_url.py
└── integration/
    └── integration.py
```

### 8.3 타입 안전성
Python wrapper에서 `TypedDict` 또는 `dataclass`로 state/lidar 등의 데이터 모델을 정의하면, IDE 자동완성 + 문서화에 유리.

### 8.4 Plugin/Extension 패턴
향후 사용자가 커스텀 토픽 핸들러를 등록할 수 있도록:
```python
conn.datachannel.register_handler("rt/custom/topic", CustomHandler())
```
이 패턴은 `datachannel/core/` 에 한 번 구현하면, 모든 도메인 모듈이 재사용 가능.
