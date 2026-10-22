"""Validate LOW_STATE subscription without physical commands."""

import asyncio
import time

from unitree_webrtc_rs import RTC_TOPIC

from example_support import argument_parser, connection, run


async def collect_state(conn, topic, duration, required_fields):
    count = 0
    latest = None
    malformed = False
    received_at = None

    def callback(message):
        nonlocal count, latest, malformed, received_at
        data = message.get("data") if isinstance(message, dict) else None
        if not isinstance(data, dict) or not all(
            key in data for key in required_fields
        ):
            malformed = True
            return
        count += 1
        latest = data
        received_at = time.monotonic()

    conn.datachannel.pub_sub.subscribe(topic, callback)
    try:
        await asyncio.sleep(duration)
        if malformed or count == 0 or time.monotonic() - received_at > 1.0:
            raise RuntimeError("No valid state stream received")
        print(f"State {topic}: {count} frames, fields={sorted(latest)}")
        return latest
    finally:
        conn.datachannel.pub_sub.unsubscribe(topic)


def parse_args():
    return argument_parser("Validate LOW_STATE").parse_args()


async def main():
    args = parse_args()
    async with connection(args) as conn:
        await collect_state(
            conn,
            RTC_TOPIC["LOW_STATE"],
            args.duration,
            ("imu_state", "motor_state", "bms_state"),
        )


if __name__ == "__main__":
    run(main)
