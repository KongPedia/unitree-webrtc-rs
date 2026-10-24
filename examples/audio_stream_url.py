"""Stream audio for a bounded duration on an explicitly selected robot."""

import asyncio
import logging

from example_support import argument_parser, connection, run


def parse_args():
    parser = argument_parser("Stream internet radio to Go2")
    parser.add_argument(
        "--url",
        default="https://nashe1.hostingradio.ru:80/ultra-128.mp3",
        help="Audio stream URL",
    )
    return parser.parse_args()


async def main():
    args = parse_args()
    async with connection(args) as conn:
        failed = False
        try:
            await asyncio.wait_for(conn.audio.play_from_url(args.url), timeout=45)
            print("Audio URL playback started")
            await asyncio.sleep(args.duration)
        except BaseException:
            failed = True
            raise
        finally:
            try:
                await asyncio.wait_for(conn.audio.stop(), timeout=12)
            except Exception:
                logging.error("Audio stop failed; cleanup may be incomplete")
                if not failed:
                    raise RuntimeError("Audio stop failed; cleanup may be incomplete") from None
    print("Audio playback stopped; disconnected")


if __name__ == "__main__":
    run(main)
