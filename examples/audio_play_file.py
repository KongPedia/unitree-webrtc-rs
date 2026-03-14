import argparse
import asyncio
import sys
from pathlib import Path

from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod


async def main():
    parser = argparse.ArgumentParser(description="Play local audio file to Go2")
    parser.add_argument("--ip", type=str, default="10.2.80.101", help="Robot IP address")
    parser.add_argument("--file", type=str, required=True, help="Local audio file path")
    parser.add_argument("--duration", type=float, default=30.0, help="Playback duration in seconds")
    args = parser.parse_args()

    file_path = Path(args.file).expanduser().resolve()
    if not file_path.exists():
        raise FileNotFoundError(f"File not found: {file_path}")

    print(f"Connecting to robot at {args.ip}...")
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip=args.ip)
    await conn.connect()
    print("Connected!")

    await conn.audio.play_from_file(str(file_path))
    print(f"Playing file: {file_path}")

    try:
        await asyncio.sleep(args.duration)
    finally:
        await conn.audio.stop()
        await conn.disconnect()
        print("Disconnected")


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("\nInterrupted by user")
        sys.exit(0)
