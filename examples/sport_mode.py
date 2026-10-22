"""Read-only by default; explicit consent enables guarded posture actions."""

import asyncio
import json
import math

from unitree_webrtc_rs import RTC_TOPIC, SPORT_CMD

from example_support import (
    UnsupportedPostureModeError,
    argument_parser,
    connection,
    response_data,
    run,
)
from lowstate_test import collect_state

# Go2 motion-switcher read API; this is not the sport Damp API with the same ID.
MOTION_SWITCHER_GET_MODE = 1001


async def query_state(conn):
    reply = await conn.datachannel.pub_sub.publish_request_new(
        RTC_TOPIC["SPORT_MOD"], {"api_id": SPORT_CMD["GetState"]}, timeout=10
    )
    response_data(reply)
    print("GetState status code: 0")


def finite_number(value):
    return type(value) in (int, float) and math.isfinite(value)


def body_height(state):
    height = state.get("body_height")
    if not finite_number(height) or not 0 <= height <= 1:
        raise ValueError("Invalid body height; refusing motion")
    return height


def require_live_session(conn):
    age = conn.datachannel.last_heartbeat_age_s
    if (
        not conn.isConnected
        or not conn.datachannel.heartbeat_running
        or type(age) is not int
        or age > 3
    ):
        raise ValueError(
            "Fresh heartbeat and connected session required; refusing motion"
        )


async def observe_posture(conn, duration):
    low, sport = await asyncio.gather(
        collect_state(conn, RTC_TOPIC["LOW_STATE"], duration, ("imu_state",)),
        collect_state(
            conn, RTC_TOPIC["LF_SPORT_MOD_STATE"], duration, ("body_height", "mode")
        ),
    )
    rpy = low["imu_state"].get("rpy") if isinstance(low["imu_state"], dict) else None
    if (
        not isinstance(rpy, (list, tuple))
        or len(rpy) != 3
        or not all(finite_number(value) for value in rpy)
    ):
        raise ValueError("Fresh finite orientation required; refusing motion")
    if abs(rpy[0]) > 0.5 or abs(rpy[1]) > 0.5:
        raise ValueError("Unsafe orientation; refusing motion")
    require_live_session(conn)
    return body_height(sport), sport["mode"]


async def posture_sequence(conn, duration, posture="cycle"):
    reply = await conn.datachannel.pub_sub.publish_request_new(
        RTC_TOPIC["MOTION_SWITCHER"], {"api_id": MOTION_SWITCHER_GET_MODE}, timeout=10
    )
    mode = response_data(reply)
    if isinstance(mode, str):
        mode = json.loads(mode)
    # Only StandUp/StandDown are verified for both modes; do not generalize other APIs.
    if not isinstance(mode, dict) or mode.get("name") not in ("normal", "mcf"):
        raise UnsupportedPostureModeError(
            "Normal or MCF motion mode required; this example does not switch modes"
        )
    baseline, _ = await observe_posture(conn, duration)
    if posture != "stand-down" and baseline > 0.2:
        raise ValueError("Start in a lowered posture for this validation sequence")
    if posture == "stand-down" and baseline <= 0.2:
        raise ValueError("Start standing for single StandDown validation")
    commands = {
        "cycle": ("StandUp", "StandDown"),
        "stand-up": ("StandUp",),
        "stand-down": ("StandDown",),
    }[posture]
    if posture == "stand-up":
        print("Explicit StandUp: robot will remain standing; run StandDown separately")
    previous_height = baseline
    confirmed = []
    for name in commands:
        require_live_session(conn)
        reply = await conn.datachannel.pub_sub.publish_request_new(
            RTC_TOPIC["SPORT_MOD"], {"api_id": SPORT_CMD[name]}, timeout=10
        )
        response_data(reply)
        height, mode = await observe_posture(conn, duration)
        delta = (
            height - previous_height if name == "StandUp" else previous_height - height
        )
        confirmed.append(delta >= 0.1)
        previous_height = height
        print(
            f"{name}: status=0, body_height={height}, mode={mode}, observed_delta={delta:.3f}m"
        )
        # An ACK without an upward delta is not proof, but safe gates still permit the requested lowering.
    if not all(confirmed) or (posture != "stand-up" and previous_height > 0.2):
        raise RuntimeError("Posture telemetry did not confirm the requested movement")


def parse_args(argv=None):
    parser = argument_parser("Query state; optionally StandUp/StandDown only")
    parser.add_argument(
        "--allow-motion",
        action="store_true",
        help="Explicitly permit the selected posture action",
    )
    parser.add_argument(
        "--posture",
        choices=("cycle", "stand-up", "stand-down"),
        default="cycle",
        help="Cycle finishes lowered; stand-up intentionally leaves the robot standing",
    )
    args = parser.parse_args(argv)
    if args.posture != "cycle" and not args.allow_motion:
        parser.error("--posture requires --allow-motion")
    return args


async def main():
    args = parse_args()
    async with connection(args) as conn:
        await query_state(conn)
        if args.allow_motion:
            await posture_sequence(conn, args.duration, args.posture)
        else:
            await asyncio.sleep(args.duration)


if __name__ == "__main__":
    run(main)
