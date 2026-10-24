"""Bounded audio reception with optional, lazily imported PyAudio playback."""

import asyncio
import logging
import time
from queue import Empty, Full, Queue

import numpy as np
from example_support import argument_parser, connection, run

# Example-local retention ceilings, not robot/codec limits. Overflow drops newest.
MAX_QUEUED_FRAMES = 4
MAX_FRAME_SAMPLES = 96_000


async def write_audio(stream, samples):
    # Keep ownership until a blocking write completes, even if its waiter cancels.
    task = asyncio.create_task(asyncio.to_thread(stream.write, samples.tobytes()))
    cancelled = False
    while not task.done():
        try:
            await asyncio.shield(task)
        except asyncio.CancelledError:
            cancelled = True
        except Exception:
            if not cancelled:
                raise
    if cancelled:
        if not task.cancelled() and task.exception() is not None:
            logging.error("Audio playback write failed during cancellation")
        raise asyncio.CancelledError
    task.result()


async def collect_audio(conn, duration):
    frames = Queue(maxsize=MAX_QUEUED_FRAMES)
    received = 0
    dropped = 0
    malformed = False
    accepting = True
    player = None
    stream = None
    failed = False

    def callback(samples):
        nonlocal received, dropped, malformed
        if not accepting:
            return
        if (
            not isinstance(samples, np.ndarray)
            or samples.dtype != np.int16
            or samples.ndim != 1
            or not 0 < samples.size <= MAX_FRAME_SAMPLES
        ):
            malformed = True
            return
        received += 1
        try:
            # Native arrays own their storage; retain the bounded frame, no copy.
            frames.put_nowait(samples)
        except Full:
            dropped += 1

    try:
        conn.audio.on_audio(callback)
        conn.audio.switchAudioChannel(True)
        try:
            import pyaudio
        except ImportError:
            print("PyAudio unavailable; analyzing audio without playback")
        else:
            player = pyaudio.PyAudio()
            stream = player.open(
                format=pyaudio.paInt16,
                channels=2,
                rate=48000,
                output=True,
                frames_per_buffer=1920,
            )
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            try:
                samples = frames.get_nowait()
            except Empty:
                pass
            else:
                if stream is not None:
                    await write_audio(stream, samples)
            await asyncio.sleep(0.01)
        if malformed or received == 0:
            raise RuntimeError("No valid audio stream received")
        print(f"Audio: {received} frames, dropped={dropped}, dtype=int16")
        return received
    except BaseException:
        failed = True
        raise
    finally:
        accepting = False
        closers = [("audio channel disable", lambda: conn.audio.switchAudioChannel(False))]
        if stream is not None:
            closers.extend(
                (
                    ("playback stop", stream.stop_stream),
                    ("playback close", stream.close),
                )
            )
        if player is not None:
            closers.append(("PyAudio terminate", player.terminate))
        cleanup_failed = False
        for label, close in closers:
            try:
                close()
            except Exception:
                cleanup_failed = True
                logging.error("Audio cleanup failed: %s", label)
        while not frames.empty():
            frames.get_nowait()
        if cleanup_failed and not failed:
            raise RuntimeError("Audio cleanup failed; cleanup may be incomplete") from None


def parse_args():
    return argument_parser("Receive and analyze native int16 audio").parse_args()


async def main():
    args = parse_args()
    async with connection(args) as conn:
        await collect_audio(conn, args.duration)


if __name__ == "__main__":
    run(main)
