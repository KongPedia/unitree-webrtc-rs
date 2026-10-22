"""Shared example configuration and bounded connection cleanup, not a robot SDK."""

import argparse
import asyncio
import logging
import math
import os
from contextlib import asynccontextmanager
from pathlib import Path

from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod


class UnsupportedPostureModeError(ValueError):
    """A verified posture mode is required; never echo the peer's mode text."""


class LidarStreamValidationError(RuntimeError):
    """Missing, malformed or stale points do not prove a transport failure."""


def positive_seconds(value):
    seconds = float(value)
    if not math.isfinite(seconds) or not 0 < seconds <= 3600:
        raise argparse.ArgumentTypeError("seconds must be finite and in (0, 3600]")
    return seconds


def robot_address(value):
    address = value.strip()
    if not address or any(character.isspace() for character in address):
        raise argparse.ArgumentTypeError(
            "Robot address must be nonempty with no whitespace"
        )
    return address


def argument_parser(description):
    parser = argparse.ArgumentParser(description=description)
    ip = os.getenv("ROBOT_IP")
    parser.add_argument(
        "--ip",
        type=robot_address,
        default=ip,
        required=not ip,
        help="Robot address or ROBOT_IP",
    )
    parser.add_argument("--duration", type=positive_seconds, default=5.0)
    parser.add_argument(
        "--aes-128-key", help="Prefer key file/environment; argv is visible"
    )
    parser.add_argument(
        "--aes-128-key-file",
        default=os.getenv("UNITREE_AES_128_KEY_FILE")
        or os.getenv("GO2_DORA_UNITREE_AES_128_KEY_FILE"),
        help="Path to per-device AES key (use '-' to read stdin)",
    )
    return parser


def resolve_aes_key(args):
    if args.aes_128_key:
        return args.aes_128_key.strip()
    if args.aes_128_key_file:
        if args.aes_128_key_file == "-":
            import sys

            return sys.stdin.readline().strip()
        return Path(args.aes_128_key_file).expanduser().read_text().strip()
    value = os.getenv("UNITREE_AES_128_KEY") or os.getenv(
        "GO2_DORA_UNITREE_AES_128_KEY"
    )
    return value.strip() if value else None


@asynccontextmanager
async def connection(args):
    conn = UnitreeWebRTCConnection(
        WebRTCConnectionMethod.LocalSTA, ip=args.ip, aes_128_key=resolve_aes_key(args)
    )
    failed = False
    try:
        await asyncio.wait_for(conn.connect(), timeout=45)
        yield conn
    except BaseException:
        failed = True
        raise
    finally:
        try:
            await asyncio.wait_for(conn.disconnect(), timeout=12)
        except Exception:
            # Do not print transport error payloads; they may contain device secrets.
            logging.error("Disconnect failed; cleanup may be incomplete")
            if not failed:
                raise RuntimeError(
                    "Disconnect failed; cleanup may be incomplete"
                ) from None


def response_data(response):
    try:
        data = response["data"]
        code = data["header"]["status"]["code"]
    except (KeyError, TypeError) as error:
        raise ValueError("Malformed robot response") from error
    if type(code) is not int or code != 0:
        raise ValueError("Robot request failed")
    return data.get("data")


def run(main):
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        raise SystemExit(130) from None
    except Exception as error:
        if isinstance(error, UnsupportedPostureModeError):
            reason = "unsupported_posture_mode"
        elif isinstance(error, LidarStreamValidationError):
            reason = "lidar_stream_missing_invalid_or_stale"
        else:
            reason = "unclassified"
        logging.error("Example failed; reason=%s", reason)
        raise SystemExit(1) from None
