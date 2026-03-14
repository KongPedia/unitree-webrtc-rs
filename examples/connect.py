import argparse
import asyncio
import os

from unitree_webrtc_rs.webrtc_driver import (
    UnitreeWebRTCConnection,
    WebRTCConnectionMethod,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="unitree_webrtc_rs connect test")
    parser.add_argument("--ip", default=os.getenv("ROBOT_IP", "10.2.80.114"))
    parser.add_argument("--duration", type=float, default=5.0)
    return parser.parse_args()


async def main() -> None:
    args = parse_args()

    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip=args.ip)
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
