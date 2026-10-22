"""Bounded native integration checks. No audio or physical motion by default."""

import asyncio

import numpy as np
from unitree_webrtc_rs import RTC_TOPIC

from example_support import argument_parser, connection, run
from lidar_stream import collect_lidar
from lowstate_test import collect_state
from sport_mode import posture_sequence, query_state
from video_stream import collect_video


async def collect_audio(conn, duration):
    count = 0
    malformed = False

    def callback(samples):
        nonlocal count, malformed
        if (
            not isinstance(samples, np.ndarray)
            or samples.dtype != np.int16
            or samples.ndim != 1
            or samples.size == 0
        ):
            malformed = True
            return
        count += 1

    conn.audio.on_audio(callback)
    try:
        conn.audio.switchAudioChannel(True)
        await asyncio.sleep(duration)
        if malformed or count == 0:
            raise RuntimeError("No valid audio stream received")
        print(f"Audio: {count} frames, dtype=int16")
    finally:
        conn.audio.switchAudioChannel(False)


def parse_args():
    parser = argument_parser("Check connection, state, LiDAR and camera")
    parser.add_argument(
        "--allow-motion", action="store_true", help="Also StandUp then StandDown"
    )
    parser.add_argument(
        "--include-audio",
        action="store_true",
        help="Opt in only on an audio-capable robot",
    )
    parser.add_argument("--output", help="Save a validated camera image")
    return parser.parse_args()


async def main():
    args = parse_args()
    async with connection(args) as conn:
        await query_state(conn)
        await collect_state(
            conn,
            RTC_TOPIC["LOW_STATE"],
            args.duration,
            ("imu_state", "motor_state", "bms_state"),
        )
        await collect_state(
            conn,
            RTC_TOPIC["LF_SPORT_MOD_STATE"],
            args.duration,
            ("body_height", "mode", "imu_state"),
        )
        await collect_lidar(conn, args.duration)
        await collect_video(conn, args.duration, output=args.output)
        if args.include_audio:
            await collect_audio(conn, args.duration)
        else:
            print("Audio: skipped (not requested)")
        if args.allow_motion:
            await posture_sequence(conn, args.duration)
    print("Integration checks passed; connection closed")


if __name__ == "__main__":
    run(main)
