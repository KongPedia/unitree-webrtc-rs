"""Connect and cleanly disconnect; no motion or media commands."""

import asyncio

from example_support import argument_parser, connection, run


def parse_args():
    return argument_parser("Connect to a Unitree robot").parse_args()


async def main():
    args = parse_args()
    async with connection(args) as conn:
        if not conn.isConnected:
            raise RuntimeError("Connection not established")
        print("Connected: True")
        await asyncio.sleep(args.duration)
    print(f"Connected after disconnect: {conn.isConnected}")
    if conn.isConnected:
        raise RuntimeError("Connection did not close")


if __name__ == "__main__":
    run(main)
