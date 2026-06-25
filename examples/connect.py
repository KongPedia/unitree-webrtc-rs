import argparse
import asyncio
import os
from pathlib import Path

from unitree_webrtc_rs.webrtc_driver import (
    UnitreeWebRTCConnection,
    WebRTCConnectionMethod,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="unitree_webrtc_rs connect test")
    parser.add_argument("--ip", default=os.getenv("ROBOT_IP", "10.2.80.114"))
    parser.add_argument(
        "--aes-128-key",
        default=None,
        help=(
            "32-character per-device AES-128 hex key required by Go2 firmware 1.1.15+ LAN handshakes. "
            "Prefer environment variables or --aes-128-key-file for real keys because argv can be visible."
        ),
    )
    parser.add_argument(
        "--aes-128-key-file",
        default=os.getenv("GO2_DORA_UNITREE_AES_128_KEY_FILE") or os.getenv("UNITREE_AES_128_KEY_FILE"),
        help="Path to a file containing the per-device AES key; preferred over passing the key as argv.",
    )
    parser.add_argument("--duration", type=float, default=5.0)
    return parser.parse_args()


def _resolve_aes_key(args: argparse.Namespace) -> str | None:
    if args.aes_128_key:
        return args.aes_128_key.strip()
    if args.aes_128_key_file:
        file_key = Path(args.aes_128_key_file).expanduser().read_text().strip()
        if file_key:
            return file_key
    env_key = os.getenv("GO2_DORA_UNITREE_AES_128_KEY") or os.getenv("UNITREE_AES_128_KEY")
    return env_key.strip() if env_key else None


async def main() -> None:
    args = parse_args()
    aes_128_key = _resolve_aes_key(args)

    conn = UnitreeWebRTCConnection(
        WebRTCConnectionMethod.LocalSTA,
        ip=args.ip,
        aes_128_key=aes_128_key,
    )
    # conn = UnitreeWebRTCConnection(
    #     WebRTCConnectionMethod.LocalSTA, serialNumber="B42D2000XXXXXXXX"
    # )
    # conn = UnitreeWebRTCConnection(
    #     WebRTCConnectionMethod.Remote,
    #     serialNumber="B42D2000XXXXXXXX",
    #     username="email@gmail.com",
    #     password="pass",
    # )
    # conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalAP)

    print(f"Connecting to {args.ip}...")
    await conn.connect()
    print(f"Connected: {conn.isConnected}")

    await asyncio.sleep(args.duration)

    print("Disconnecting...")
    await conn.disconnect()
    print(f"Connected after disconnect: {conn.isConnected}")


if __name__ == "__main__":
    asyncio.run(main())
