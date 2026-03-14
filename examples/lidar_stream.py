import argparse
import asyncio
import logging
import os
import sys

from unitree_webrtc_rs.webrtc_driver import (
    UnitreeWebRTCConnection,
    WebRTCConnectionMethod,
)

logging.basicConfig(level=logging.FATAL)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="unitree_webrtc_rs lidar example")
    parser.add_argument("--ip", default=os.getenv("ROBOT_IP", "10.2.80.114"))
    parser.add_argument("--duration", type=float, default=10.0)
    return parser.parse_args()


async def main() -> None:
    args = parse_args()

    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip=args.ip)

    try:
        await conn.connect()
        await conn.datachannel.disableTrafficSaving(True)

        conn.datachannel.set_decoder(decoder_type="native")

        conn.datachannel.pub_sub.publish_without_callback("rt/utlidar/switch", "on")

        frame_count = 0

        def lidar_callback(message):
            nonlocal frame_count
            frame_count += 1
            if frame_count == 1:
                print("\n[Rust unitree_webrtc_rs] First frame data format:")
                print(f"  message type: {type(message)}")
                print(f"  message keys: {list(message.keys()) if isinstance(message, dict) else 'N/A'}")
                data = message.get("data", {})
                print(f"  data type: {type(data)}")
                print(f"  data keys: {list(data.keys()) if isinstance(data, dict) else 'N/A'}")
                points_data = data.get("data")
                print(f"  data['data'] exists: {points_data is not None}")
                if points_data is not None:
                    print(f"  data['data'] type: {type(points_data)}")
                    if isinstance(points_data, dict):
                        print(f"  data['data'] keys: {list(points_data.keys())}")
                        print(f"  data['data'] full content: {points_data}")
                        points = points_data.get("points")
                        print(f"  points exists: {points is not None}")
                        if points is not None:
                            import numpy as np
                            if isinstance(points, np.ndarray):
                                print(f"  points type: numpy.ndarray")
                                print(f"  points shape: {points.shape}")
                                print(f"  points dtype: {points.dtype}")
                                print(f"  points sample (first 3): {points[:3].tolist() if len(points) > 0 else 'empty'}")
                            else:
                                print(f"  points type: {type(points)} (unexpected)")
                    else:
                        import numpy as np
                        if isinstance(points_data, np.ndarray):
                            print(f"  data['data'] is numpy.ndarray (direct)")
                            print(f"  data['data'] shape: {points_data.shape}")
                            print(f"  data['data'] dtype: {points_data.dtype}")
                            print(f"  data['data'] sample (first 3): {points_data[:3].tolist() if len(points_data) > 0 else 'empty'}")
                        else:
                            print(f"  data['data'] type: {type(points_data)} (unexpected)")
                print()

        conn.datachannel.pub_sub.subscribe("rt/utlidar/voxel_map_compressed", lidar_callback)

        print(f"Listening for LiDAR data for {args.duration}s...")
        await asyncio.sleep(args.duration)

        print(f"\nReceived {frame_count} LiDAR frames")

    except ValueError as error:
        logging.error(f"An error occurred: {error}")
    finally:
        try:
            await conn.disconnect()
        except Exception:
            pass


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("\nProgram interrupted by user")
        sys.exit(0)
