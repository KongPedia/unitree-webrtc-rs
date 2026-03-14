import argparse
import asyncio
import sys

from unitree_webrtc_rs import UnitreeWebRTCConnection, WebRTCConnectionMethod


async def main():
    parser = argparse.ArgumentParser(description="Stream internet radio to Go2")
    parser.add_argument("--ip", type=str, default="10.2.80.101", help="Robot IP address")
    parser.add_argument(
        "--url",
        type=str,
        default="https://nashe1.hostingradio.ru:80/ultra-128.mp3",
        help="Audio stream URL",
    )
    parser.add_argument("--duration", type=float, default=30.0, help="Playback duration in seconds")
    args = parser.parse_args()

    print(f"Connecting to robot at {args.ip}...")
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip=args.ip)
    await conn.connect()
    print("Connected!")

    await conn.audio.play_from_url(args.url)
    print(f"Streaming URL: {args.url}")

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
