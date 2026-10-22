"""Validate native LiDAR clouds for a bounded duration; leave the sensor enabled."""

import asyncio
import time

import numpy as np
from unitree_webrtc_rs import RTC_TOPIC

from example_support import LidarStreamValidationError, argument_parser, connection, run


def validate_points(message):
    points = message["data"]["data"]["points"]
    if not isinstance(points, np.ndarray) or points.dtype != np.float32:
        raise ValueError("LiDAR must be a float32 NumPy array")
    if points.ndim != 2 or points.shape[1] != 3 or points.shape[0] == 0:
        raise ValueError("LiDAR must have nonempty shape (N, 3)")
    if not np.isfinite(points).all():
        raise ValueError("LiDAR contains nonfinite coordinates")
    return points


async def collect_lidar(conn, duration):
    count = 0
    failure = None
    first_shape = None
    received_at = None

    def callback(message):
        nonlocal count, failure, first_shape, received_at
        try:
            points = validate_points(message)
            count += 1
            received_at = time.monotonic()
            if first_shape is None:
                first_shape = points.shape
        except (KeyError, TypeError, ValueError) as error:
            failure = type(error).__name__

    topic = RTC_TOPIC["ULIDAR_ARRAY"]
    conn.datachannel.pub_sub.subscribe(topic, callback)
    try:
        if not await conn.datachannel.disableTrafficSaving(True):
            raise RuntimeError("Traffic-saving request was not accepted")
        conn.datachannel.set_decoder(decoder_type="native")
        conn.datachannel.pub_sub.publish_without_callback(
            RTC_TOPIC["ULIDAR_SWITCH"], "on"
        )
        await asyncio.sleep(duration)
        if failure or count == 0 or time.monotonic() - received_at > 1.0:
            raise LidarStreamValidationError("No valid LiDAR stream received")
        print(f"LiDAR: {count} frames, first shape={first_shape}, dtype=float32")
        return count
    finally:
        conn.datachannel.pub_sub.unsubscribe(topic)


def parse_args():
    return argument_parser("Validate native LiDAR data").parse_args()


async def main():
    args = parse_args()
    async with connection(args) as conn:
        await collect_lidar(conn, args.duration)


if __name__ == "__main__":
    run(main)
