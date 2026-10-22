"""Receive sport telemetry without changing modes or moving the robot."""

from unitree_webrtc_rs import RTC_TOPIC

from example_support import argument_parser, connection, run
from lowstate_test import collect_state


def parse_args():
    return argument_parser("Validate sport state telemetry").parse_args()


async def main():
    args = parse_args()
    async with connection(args) as conn:
        state = await collect_state(
            conn,
            RTC_TOPIC["LF_SPORT_MOD_STATE"],
            args.duration,
            ("body_height", "mode", "imu_state"),
        )
        print(f"SportState: mode={state['mode']}, body_height={state['body_height']}")


if __name__ == "__main__":
    run(main)
