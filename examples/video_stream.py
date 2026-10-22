"""Bounded video reception on the owning Python thread, with optional headless capture."""

import asyncio
import time

import numpy as np

from example_support import argument_parser, connection, run


def validate_frame(frame):
    if not isinstance(frame, np.ndarray) or frame.dtype != np.uint8:
        raise ValueError("Video must be a uint8 NumPy array")
    if frame.ndim != 3 or frame.shape[2] != 3 or min(frame.shape[:2]) == 0:
        raise ValueError("Video must have nonempty BGR shape (H, W, 3)")
    return frame


async def collect_video(conn, duration, *, headless=True, output=None):
    count = 0
    latest = None
    failure = None
    received_at = None
    cv2 = None
    if not headless or output:
        import cv2

    def callback(frame):
        nonlocal count, latest, failure, received_at
        try:
            latest = validate_frame(frame)
            count += 1
            received_at = time.monotonic()
        except ValueError:
            failure = True

    conn.video.on_frame(callback)
    try:
        conn.video.switchVideoChannel(True)
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            await asyncio.sleep(0.05)
            if not headless and latest is not None:
                cv2.imshow("Unitree BGR video", latest)
                if cv2.waitKey(1) & 0xFF == ord("q"):
                    break
        if failure or count == 0 or time.monotonic() - received_at > 1.0:
            raise RuntimeError("No valid video stream received")
        if output and not cv2.imwrite(str(output), latest):
            raise RuntimeError("Camera image could not be written")
        print(f"Video: {count} frames, shape={latest.shape}, dtype=uint8 BGR")
        return count
    finally:
        conn.video.switchVideoChannel(False)
        if not headless and cv2 is not None:
            cv2.destroyAllWindows()


def parse_args():
    parser = argument_parser("Stream native BGR video")
    parser.add_argument("--headless", action="store_true", help="Do not open a GUI")
    parser.add_argument(
        "--output", help="Save the last validated BGR frame as an image"
    )
    return parser.parse_args()


async def main():
    args = parse_args()
    async with connection(args) as conn:
        await collect_video(
            conn, args.duration, headless=args.headless, output=args.output
        )


if __name__ == "__main__":
    run(main)
