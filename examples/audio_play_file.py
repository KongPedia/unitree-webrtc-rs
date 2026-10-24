"""Play a local audio file for a bounded duration on an explicitly selected robot."""

import asyncio
import logging
from pathlib import Path

from example_support import argument_parser, connection, run


def parse_args():
    parser = argument_parser("Play local audio file to Go2")
    parser.add_argument("--file", required=True, help="Local audio file path")
    return parser.parse_args()


async def main():
    args = parse_args()
    file_path = Path(args.file).expanduser().resolve()
    if not file_path.is_file():
        raise FileNotFoundError("Audio file is not a regular file")

    async with connection(args) as conn:
        failed = False
        try:
            await asyncio.wait_for(conn.audio.play_from_file(str(file_path)), timeout=45)
            print("Audio file playback started")
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
