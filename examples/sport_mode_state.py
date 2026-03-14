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

RTC_TOPIC = {
    "LF_SPORT_MOD_STATE": "rt/lf/sportmodestate",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="unitree_webrtc_rs sportmodestate example")
    parser.add_argument("--ip", default=os.getenv("ROBOT_IP", "10.2.80.114"))
    parser.add_argument("--duration", type=float, default=3600.0)
    return parser.parse_args()


def display_data(message: dict) -> None:
    imu_state = message["imu_state"]
    quaternion = imu_state["quaternion"]
    gyroscope = imu_state["gyroscope"]
    accelerometer = imu_state["accelerometer"]
    rpy = imu_state["rpy"]
    temperature = imu_state["temperature"]

    mode = message["mode"]
    progress = message["progress"]
    gait_type = message["gait_type"]
    foot_raise_height = message["foot_raise_height"]
    position = message["position"]
    body_height = message["body_height"]
    velocity = message["velocity"]
    yaw_speed = message["yaw_speed"]
    range_obstacle = message["range_obstacle"]
    foot_force = message["foot_force"]
    foot_position_body = message["foot_position_body"]
    foot_speed_body = message["foot_speed_body"]

    sys.stdout.write("\033[H\033[J")

    print("Go2 Robot Status")
    print("===================")
    print(f"Mode: {mode}")
    print(f"Progress: {progress}")
    print(f"Gait Type: {gait_type}")
    print(f"Foot Raise Height: {foot_raise_height} m")
    print(f"Position: {position}")
    print(f"Body Height: {body_height} m")
    print(f"Velocity: {velocity}")
    print(f"Yaw Speed: {yaw_speed}")
    print(f"Range Obstacle: {range_obstacle}")
    print(f"Foot Force: {foot_force}")
    print(f"Foot Position (Body): {foot_position_body}")
    print(f"Foot Speed (Body): {foot_speed_body}")
    print("-------------------")
    print(f"IMU - Quaternion: {quaternion}")
    print(f"IMU - Gyroscope: {gyroscope}")
    print(f"IMU - Accelerometer: {accelerometer}")
    print(f"IMU - RPY: {rpy}")
    print(f"IMU - Temperature: {temperature}°C")

    sys.stdout.flush()


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

    try:
        await conn.connect()

        def sportmodestate_callback(message: dict) -> None:
            current_message = message["data"]
            if isinstance(current_message, dict):
                display_data(current_message)

        conn.datachannel.pub_sub.subscribe(
            RTC_TOPIC["LF_SPORT_MOD_STATE"],
            sportmodestate_callback,
        )

        await asyncio.sleep(args.duration)

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
