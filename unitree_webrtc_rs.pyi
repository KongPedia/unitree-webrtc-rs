"""Type stubs for unitree_webrtc_rs Rust extension module.

This file provides IDE autocomplete and type checking support.
"""

from collections.abc import Awaitable, Callable
from typing import Any

import numpy as np
import numpy.typing as npt

class WebRTCConnectionMethod:
    """WebRTC connection method constants."""

    LocalAP: int  # = 1
    LocalSTA: int  # = 2
    Remote: int  # = 3

class VUI_COLOR:
    """VUI display color constants."""

    WHITE: str  # = "white"
    RED: str  # = "red"
    YELLOW: str  # = "yellow"
    BLUE: str  # = "blue"
    GREEN: str  # = "green"
    CYAN: str  # = "cyan"
    PURPLE: str  # = "purple"

class PubSubBridge:
    """Publish-Subscribe bridge for DataChannel messaging.

    Provides methods to publish messages and subscribe to topics.
    Payloads are JSON-compatible values; API requests use dictionaries.
    """

    def publish_request_new(
        self, topic: str, options: dict[str, Any], timeout: float = 10.0
    ) -> Awaitable[dict[str, Any]]:
        """Publish a request with new-style API.

        Args:
            topic: Topic name (e.g., "rt/api/motion_switcher/request")
            options: api_id and optional parameter; Rust constructs the envelope
            timeout: Finite, nonnegative timeout in seconds

        Returns:
            Response dictionary
        """

    def publish(
        self,
        topic: str,
        data: Any = None,
        msg_type: str | None = None,
        timeout: float = 10.0,
    ) -> Awaitable[dict[str, Any]]:
        """Publish a message and wait for response.

        Args:
            topic: Topic name
            data: Message data (optional)
            msg_type: Message type (optional)
            timeout: Finite, nonnegative timeout in seconds

        Returns:
            Response dictionary
        """

    def publish_without_callback(self, topic: str, data: Any = None, msg_type: str | None = None) -> None:
        """Publish a message without waiting for response.

        Args:
            topic: Topic name
            data: Message data (optional)
            msg_type: Message type (optional)
        """

    def subscribe(self, topic: str, callback: Callable[[dict[str, Any]], None]) -> None:
        """Subscribe to a topic with a callback.

        Args:
            topic: Topic name
            callback: Function to call when messages arrive
        """

    def unsubscribe(self, topic: str) -> None:
        """Unsubscribe from a topic.

        Args:
            topic: Topic name
        """

class VideoBridge:
    """Video stream bridge.

    Controls video streaming and provides frame callbacks.
    """

    def switchVideoChannel(self, switch: bool) -> None:
        """Enable or disable video streaming.

        Args:
            switch: True to enable, False to disable
        """

    def on_frame(self, callback: Callable[[npt.NDArray[np.uint8]], None]) -> None:
        """Register video frame callback.

        Args:
            callback: Function receiving (H, W, 3) numpy array in BGR format
        """

class AudioBridge:
    """Audio stream bridge.

    Controls audio streaming and provides audio callbacks.
    """

    def switchAudioChannel(self, switch: bool) -> None:
        """Enable or disable audio streaming.

        Args:
            switch: True to enable, False to disable
        """

    def on_audio(self, callback: Callable[[npt.NDArray[np.int16]], None]) -> None:
        """Register audio frame callback.

        Args:
            callback: Function receiving PCM samples as numpy array
        """

    def play_from_file(self, path: str) -> Awaitable[tuple[()]]:
        """Play audio from file through WebRTC.

        Args:
            path: Path to audio file
        """

    def play_from_url(self, url: str) -> Awaitable[tuple[()]]:
        """Play audio from URL through WebRTC.

        Args:
            url: Audio stream URL
        """

    def stop(self) -> Awaitable[tuple[()]]:
        """Stop audio playback; resolves to ``()``."""

class DataChannelBridge:
    """DataChannel control bridge.

    Provides access to pub/sub and channel control methods.
    """

    @property
    def pub_sub(self) -> PubSubBridge:
        """Get PubSub bridge for messaging."""

    def disableTrafficSaving(self, switch: bool = True) -> Awaitable[bool]:
        """Disable traffic saving mode.

        Args:
            switch: True to disable traffic saving

        Returns:
            Whether the robot reported successful execution
        """

    def switchVideoChannel(self, switch: bool) -> None:
        """Enable or disable video channel.

        Args:
            switch: True to enable, False to disable
        """

    def switchAudioChannel(self, switch: bool) -> None:
        """Enable or disable audio channel.

        Args:
            switch: True to enable, False to disable
        """

    def set_decoder(self, decoder_type: str) -> None:
        """Set LiDAR decoder backend.

        Args:
            decoder_type: Must be ``"native"``
        """

    @property
    def decoder_name(self) -> str:
        """Current decoder backend display name."""

    @property
    def ready_state(self) -> str: ...
    @property
    def readyState(self) -> str: ...
    @property
    def is_open(self) -> bool: ...
    @property
    def isOpen(self) -> bool: ...
    @property
    def heartbeat_running(self) -> bool: ...
    @property
    def heartbeatRunning(self) -> bool: ...
    @property
    def last_heartbeat_age_s(self) -> int | None: ...
    @property
    def lastHeartbeatAgeS(self) -> int | None: ...

class UnitreeWebRTCConnection:
    """Main WebRTC connection to Unitree robot.

    This class manages the WebRTC peer connection, DataChannel,
    video/audio streams, and provides high-level API access.

    Example:
        >>> conn = UnitreeWebRTCConnection(
        ...     WebRTCConnectionMethod.LocalSTA,
        ...     ip="192.168.123.161"
        ... )
        >>> await conn.connect()
        >>> # Use conn.datachannel.pub_sub, conn.video, conn.audio
        >>> await conn.disconnect()
    """

    def __init__(
        self,
        connection_method: int,
        serial_number: str | None = None,
        ip: str | None = None,
        username: str | None = None,
        password: str | None = None,
        aes_128_key: str | None = None,
        **kwargs: Any,
    ) -> None:
        """Initialize WebRTC connection.

        Args:
            connection_method: Connection method (use WebRTCConnectionMethod constants)
            serial_number: Robot serial number (for Remote connection)
            ip: Robot IP address (for LocalAP/LocalSTA)
            username: Authentication username (optional)
            password: Authentication password (optional)
            aes_128_key: Per-device AES-128 hex key required by Go2 firmware
                1.1.15+ LAN con_notify handshakes using data2=3. If omitted,
                ``GO2_DORA_UNITREE_AES_128_KEY`` is checked.
            **kwargs: Additional connection parameters
        """

    def connect(self) -> Awaitable[tuple[()]]:
        """Establish WebRTC connection to robot.

        Raises:
            RuntimeError: If connection fails
        """

    def disconnect(self) -> Awaitable[tuple[()]]:
        """Disconnect transport, retaining callbacks for reuse; resolves to ``()``."""

    def clear_callbacks(self) -> None:
        """Release the current Python callback registrations synchronously.

        Does not disconnect, unsubscribe remote topics or disable media. An
        callback already copied for dispatch may still run; a finalizer may register another
        callback. Later registrations are allowed. For final cleanup, call this
        explicitly, await ``disconnect()``, then release the connection/bridges.
        """

    def reconnect(self) -> Awaitable[tuple[()]]:
        """Reconnect to robot after disconnection.

        Raises:
            RuntimeError: If reconnection fails
        """

    def _auto_reconnect(self, max_retries: int = 5) -> Awaitable[bool]:
        """Automatically reconnect with retries.

        Args:
            max_retries: Maximum number of retry attempts

        Returns:
            True if reconnection succeeded
        """

    @property
    def connection_method(self) -> int:
        """Get connection method."""

    @property
    def connectionMethod(self) -> int:
        """Get connection method (legacy name)."""

    @property
    def is_connected(self) -> bool:
        """Check if currently connected."""

    @property
    def isConnected(self) -> bool:
        """Check if currently connected (legacy name)."""

    @property
    def ip(self) -> str | None:
        """Get robot IP address."""

    @property
    def serial_number(self) -> str | None:
        """Get robot serial number."""

    @property
    def video(self) -> VideoBridge:
        """Get video bridge for video stream control."""

    @property
    def audio(self) -> AudioBridge:
        """Get audio bridge for audio stream control."""

    @property
    def datachannel(self) -> DataChannelBridge:
        """Get datachannel bridge for messaging."""

# Module-level constants
DATA_CHANNEL_TYPE: dict[str, str]
RTC_TOPIC: dict[str, str]
SPORT_CMD: dict[str, int]
AUDIO_API: dict[str, int]
APP_ERROR_MESSAGES: dict[str, str]
