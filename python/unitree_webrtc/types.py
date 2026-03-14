"""Type definitions for Unitree WebRTC data structures.

Provides TypedDict and dataclass definitions for IDE autocomplete and type checking.
"""

from typing import TypedDict, List, Optional
from dataclasses import dataclass


# ============================================================================
# Sport Mode State
# ============================================================================

class SportModeState(TypedDict):
    """Sport mode state data from rt/sportmodestate topic."""
    mode: int  # Current mode (0-11)
    gait_type: int  # Gait type
    position: List[float]  # [x, y, z] position
    body_height: float  # Body height
    velocity: List[float]  # [vx, vy, vyaw] velocity
    yaw_speed: float  # Yaw angular velocity
    range_obstacle: List[float]  # Obstacle distance ranges
    foot_raise_height: float  # Foot raise height
    foot_force: List[float]  # Force on each foot [FL, FR, RL, RR]
    foot_position_body: List[List[float]]  # Foot positions in body frame
    foot_speed_body: List[List[float]]  # Foot velocities in body frame


class LowState(TypedDict):
    """Low-level state data from rt/lf/lowstate topic."""
    level_flag: int  # State level flag
    frame_reserve: int  # Reserved frame counter
    sn: List[int]  # Serial number [2]
    version: List[int]  # Version [2]
    bandwidth: int  # Bandwidth usage
    imu: "IMUState"  # IMU data
    motor_state: List["MotorState"]  # Motor states [20]
    bms: "BatteryState"  # Battery management system
    foot_force: List[int]  # Foot force sensors [4]
    foot_force_est: List[int]  # Estimated foot force [4]
    tick: int  # Tick counter
    wireless_remote: List[int]  # Wireless remote data [40]
    crc: int  # CRC checksum


class IMUState(TypedDict):
    """IMU sensor data."""
    quaternion: List[float]  # Orientation quaternion [w, x, y, z]
    gyroscope: List[float]  # Angular velocity [x, y, z] rad/s
    accelerometer: List[float]  # Linear acceleration [x, y, z] m/s²
    rpy: List[float]  # Roll, pitch, yaw angles (degrees)
    temperature: int  # Temperature (raw value)


class MotorState(TypedDict):
    """Single motor state."""
    mode: int  # Motor mode
    q: float  # Position (rad)
    dq: float  # Velocity (rad/s)
    ddq: float  # Acceleration (rad/s²)
    tau_est: float  # Estimated torque (N⋅m)
    q_raw: float  # Raw position
    dq_raw: float  # Raw velocity
    ddq_raw: float  # Raw acceleration
    temperature: int  # Temperature (°C)
    lost: int  # Communication lost flag
    reserve: List[int]  # Reserved [2]


class BatteryState(TypedDict):
    """Battery management system state."""
    version_h: int  # Version high byte
    version_l: int  # Version low byte
    status: int  # Battery status
    soc: int  # State of charge (%)
    current: int  # Current (mA)
    cycle: int  # Charge cycle count
    bq_ntc: List[int]  # NTC temperature sensors [2]
    mcu_ntc: List[int]  # MCU temperature sensors [2]
    cell_vol: List[int]  # Cell voltages [15]


# ============================================================================
# LiDAR Data
# ============================================================================

@dataclass
class LidarPoint:
    """Single LiDAR point in 3D space."""
    x: float  # X coordinate (m)
    y: float  # Y coordinate (m)
    z: float  # Z coordinate (m)


class LidarData(TypedDict):
    """LiDAR point cloud data."""
    stamp: float  # Timestamp
    id: int  # LiDAR ID
    points: List[LidarPoint]  # Point cloud
    validNum: int  # Number of valid points
    origin: List[float]  # Origin offset [x, y, z]


# ============================================================================
# Motion Switcher
# ============================================================================

class MotionSwitcherRequest(TypedDict):
    """Motion switcher API request."""
    header: "RequestHeader"
    parameter: str  # JSON string: {"name": "normal"} or ""


class RequestHeader(TypedDict):
    """Request header."""
    identity: "RequestIdentity"


class RequestIdentity(TypedDict):
    """Request identity."""
    id: int  # Request ID
    api_id: int  # API ID (e.g., 1001=check, 1002=switch)


# ============================================================================
# VUI (Voice User Interface)
# ============================================================================

class VUIRequest(TypedDict):
    """VUI display request."""
    data: "VUIData"


class VUIData(TypedDict):
    """VUI display data."""
    text: str  # Display text
    timeout: int  # Display timeout (ms)
    color: str  # Color: "white", "red", "yellow", "blue", "green", "cyan", "purple"


# ============================================================================
# Response Types
# ============================================================================

class GenericResponse(TypedDict):
    """Generic API response."""
    header: "ResponseHeader"
    data: dict  # Response-specific data


class ResponseHeader(TypedDict):
    """Response header."""
    identity: "ResponseIdentity"
    status: "ResponseStatus"


class ResponseIdentity(TypedDict):
    """Response identity."""
    id: int  # Request ID
    api_id: int  # API ID


class ResponseStatus(TypedDict):
    """Response status."""
    code: int  # Status code (0 = success)
    msg: Optional[str]  # Error message if any


# ============================================================================
# Callback Event Types
# ============================================================================

class VideoFrameEvent(TypedDict):
    """Video frame callback event."""
    data: bytes  # Raw frame data (I420 format)
    width: int  # Frame width
    height: int  # Frame height


class AudioFrameEvent(TypedDict):
    """Audio frame callback event."""
    data: List[int]  # PCM samples (int16)
    sample_rate: int  # Sample rate (Hz)
    channels: int  # Number of channels


# ============================================================================
# Constants
# ============================================================================

# Sport mode commands
class SportCommand:
    """Sport mode command API IDs."""
    DAMP = 1001  # Damping mode
    STAND_UP = 1004  # Stand up
    STAND_DOWN = 1005  # Stand down
    RECOVER_STAND = 1006  # Recover to stand
    WALK = 1007  # Walk
    RUN = 1011  # Run


# Motion switcher modes
class MotionMode:
    """Motion switcher mode names."""
    NORMAL = "normal"
    AI = "ai"


# VUI colors
class VUIColor:
    """VUI display colors."""
    WHITE = "white"
    RED = "red"
    YELLOW = "yellow"
    BLUE = "blue"
    GREEN = "green"
    CYAN = "cyan"
    PURPLE = "purple"
