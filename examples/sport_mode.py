import argparse
import asyncio
import json
import logging
import os
import sys

from unitree_webrtc_rs.webrtc_driver import (
    UnitreeWebRTCConnection,
    WebRTCConnectionMethod,
)

logging.basicConfig(level=logging.FATAL)

RTC_TOPIC = {
    "SPORT_MOD": "rt/api/sport/request",
    "MOTION_SWITCHER": "rt/api/motion_switcher/request",
}

SPORT_CMD = {
    "StandUp": 1004,
    "RecoveryStand": 1006,
    "Hello": 1016,
    "FrontFlip": 1030,
    "StandDown": 1005,
    "GetState": 1034,
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="unitree_webrtc_rs sportmode example")
    parser.add_argument("--ip", default=os.getenv("ROBOT_IP", "10.2.80.114"))
    parser.add_argument("--duration", type=float, default=3600.0)
    parser.add_argument(
        "--full-sequence",
        action="store_true",
        help="Run full motion sequence (includes aggressive motion commands).",
    )
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

    try:
        await conn.connect()

        print("Checking current motion mode...")
        response = await conn.datachannel.pub_sub.publish_request_new(
            RTC_TOPIC["MOTION_SWITCHER"],
            {"api_id": 1001},
        )

        current_motion_switcher_mode = "unknown"
        if response["data"]["header"]["status"]["code"] == 0:
            payload = response["data"]["data"]
            if isinstance(payload, str):
                payload = json.loads(payload)
            current_motion_switcher_mode = payload.get("name", "unknown")
            print(f"Current motion mode: {current_motion_switcher_mode}")

        if current_motion_switcher_mode != "normal":
            print(
                f"Switching motion mode from {current_motion_switcher_mode} to 'normal'..."
            )
            await conn.datachannel.pub_sub.publish_request_new(
                RTC_TOPIC["MOTION_SWITCHER"],
                {
                    "api_id": 1002,
                    "parameter": {"name": "normal"},
                },
            )
            await asyncio.sleep(5)

        if not args.full_sequence:
            print("Running safe command path check (GetState)...")
            safe_response = await conn.datachannel.pub_sub.publish_request_new(
                RTC_TOPIC["SPORT_MOD"],
                {"api_id": SPORT_CMD["GetState"]},
            )
            if isinstance(safe_response, dict):
                code = (
                    safe_response.get("data", {})
                    .get("header", {})
                    .get("status", {})
                    .get("code")
                )
                print(f"GetState status code: {code}")

            print(f"Keeping session alive for {args.duration:.1f}s")
            await asyncio.sleep(args.duration)
            return

        print("Performing 'StandUp' movement...")
        await conn.datachannel.pub_sub.publish_request_new(
            RTC_TOPIC["SPORT_MOD"],
            {"api_id": SPORT_CMD["StandUp"]},
        )

        print("Performing 'RecoveryStand' movement...")
        await conn.datachannel.pub_sub.publish_request_new(
            RTC_TOPIC["SPORT_MOD"],
            {"api_id": SPORT_CMD["RecoveryStand"]},
        )
        await asyncio.sleep(1)

        print("Performing 'Hello' movement...")
        await conn.datachannel.pub_sub.publish_request_new(
            RTC_TOPIC["SPORT_MOD"],
            {"api_id": SPORT_CMD["Hello"]},
        )

        print("Switching to FrontFlip mode...")
        await conn.datachannel.pub_sub.publish_request_new(
            RTC_TOPIC["SPORT_MOD"],
            {
                "api_id": SPORT_CMD["FrontFlip"],
                "parameter": {"data": True},
            },
        )

        print("Switching to StandDown mode...")
        await conn.datachannel.pub_sub.publish_request_new(
            RTC_TOPIC["SPORT_MOD"],
            {
                "api_id": SPORT_CMD["StandDown"],
                "parameter": {"data": True},
            },
        )

        print(f"Keeping session alive for {args.duration:.1f}s")
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
