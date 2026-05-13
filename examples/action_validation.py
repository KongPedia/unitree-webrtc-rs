import argparse
import asyncio
import json
import os
import statistics
import time
from collections import Counter
from typing import Any

from unitree_webrtc_rs.webrtc_driver import UnitreeWebRTCConnection, WebRTCConnectionMethod

RTC_TOPIC = {
    "LOW_STATE": "rt/lf/lowstate",
    "LF_SPORT_MOD_STATE": "rt/lf/sportmodestate",
    "MOTION_SWITCHER": "rt/api/motion_switcher/request",
    "SPORT_MOD": "rt/api/sport/request",
    "VUI": "rt/api/vui/request",
    "WIRELESS_CONTROLLER": "rt/wirelesscontroller",
}

SPORT_CMD = {
    "Damp": 1001,
    "StandUp": 1004,
    "StandDown": 1005,
    "RecoveryStand": 1006,
}

MOTION_SWITCHER_SET_MODE = 1002
VUI_SET_BRIGHTNESS = 1005
VUI_SET_COLOR = 1007


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Validate outgoing WebRTC command/action paths with unitree-webrtc-rs Python/PyO3 binding."
    )
    parser.add_argument("--ip", default=os.getenv("ROBOT_IP", "10.2.80.114"))
    parser.add_argument(
        "--posture-observe-s",
        type=float,
        default=6.0,
        help="Seconds to observe LF_SPORT_MOD_STATE after each optional physical posture action.",
    )
    parser.add_argument("--command-timeout", type=float, default=5.0)
    parser.add_argument("--state-wait", type=float, default=2.0)
    parser.add_argument("--multi-connect", action="store_true")
    parser.add_argument("--only-multi-connect", action="store_true")
    parser.add_argument("--multi-attempts", type=int, default=1)
    parser.add_argument("--skip-vui", action="store_true")
    parser.add_argument("--vui-brightness", type=int, default=10)
    parser.add_argument("--vui-color", default="yellow")
    parser.add_argument("--vui-time", type=int, default=10)
    parser.add_argument("--vui-flash-cycle", type=int, default=None)
    parser.add_argument(
        "--vui-hold",
        type=float,
        default=2.0,
        help="Seconds to wait after the VUI color request so visual changes are observable.",
    )
    parser.add_argument(
        "--posture-sequence",
        nargs="*",
        default=[],
        choices=sorted(SPORT_CMD),
        help="Optional physical posture actions. Example: RecoveryStand StandDown StandUp RecoveryStand",
    )
    return parser.parse_args()


def print_event(name: str, payload: Any = None) -> None:
    if payload is None:
        print(name, flush=True)
        return
    if isinstance(payload, str):
        print(name, payload, flush=True)
        return
    print(name, json.dumps(payload, ensure_ascii=False, sort_keys=True), flush=True)


def response_summary(response: dict[str, Any]) -> dict[str, Any]:
    header = response.get("data", {}).get("header", {})
    identity = header.get("identity", {})
    status = header.get("status", {})
    return {
        "topic": response.get("topic"),
        "type": response.get("type"),
        "id": identity.get("id"),
        "api_id": identity.get("api_id"),
        "status_code": status.get("code"),
    }


async def send_request(
    conn: UnitreeWebRTCConnection,
    *,
    label: str,
    topic: str,
    payload: dict[str, Any],
    timeout: float,
) -> dict[str, Any]:
    started_at = time.perf_counter()
    response = await conn.datachannel.pub_sub.publish_request_new(topic, payload, timeout)
    elapsed_ms = (time.perf_counter() - started_at) * 1000.0
    summary = response_summary(response)
    summary.update({"label": label, "elapsed_ms": round(elapsed_ms, 2)})
    print_event("COMMAND_OK", summary)
    return summary


def send_zero_cmd_vel(conn: UnitreeWebRTCConnection, *, label: str) -> None:
    started_at = time.perf_counter()
    conn.datachannel.pub_sub.publish_without_callback(
        RTC_TOPIC["WIRELESS_CONTROLLER"],
        {"lx": 0.0, "ly": 0.0, "rx": 0.0, "ry": 0.0},
    )
    elapsed_ms = (time.perf_counter() - started_at) * 1000.0
    print_event("COMMAND_OK", {"label": label, "type": "zero_cmd_vel", "elapsed_ms": round(elapsed_ms, 2)})


async def connect(ip: str, label: str) -> UnitreeWebRTCConnection:
    conn = UnitreeWebRTCConnection(WebRTCConnectionMethod.LocalSTA, ip=ip)
    started_at = time.perf_counter()
    await conn.connect()
    print_event("CONNECT_OK", {"label": label, "elapsed_s": round(time.perf_counter() - started_at, 3)})
    return conn


async def disconnect_safely(conn: UnitreeWebRTCConnection | None, label: str) -> None:
    if conn is None:
        return
    try:
        await conn.disconnect()
        print_event("DISCONNECT_OK", {"label": label})
    except Exception as exc:  # noqa: BLE001 - diagnostic script should keep collecting evidence.
        print_event("DISCONNECT_ERROR", {"label": label, "error": str(exc)})


async def run_single_action_validation(args: argparse.Namespace) -> None:
    conn = await connect(args.ip, "primary")
    state_counts: Counter[str] = Counter()
    last_state: dict[str, Any] = {}

    def state_callback(message: dict[str, Any]) -> None:
        data = message.get("data", {})
        state_counts["frames"] += 1
        if isinstance(data, dict):
            last_state.clear()
            last_state.update(
                {
                    "mode": data.get("mode"),
                    "progress": data.get("progress"),
                    "gait_type": data.get("gait_type"),
                    "body_height": data.get("body_height"),
                }
            )

    try:
        conn.datachannel.pub_sub.subscribe(RTC_TOPIC["LF_SPORT_MOD_STATE"], state_callback)
        await asyncio.sleep(args.state_wait)
        print_event("STATE_OBSERVED", {"frames": state_counts["frames"], "last": last_state})

        send_zero_cmd_vel(conn, label="pre_action_zero_cmd_vel")
        await send_request(
            conn,
            label="motion_switcher_set_normal",
            topic=RTC_TOPIC["MOTION_SWITCHER"],
            payload={"api_id": MOTION_SWITCHER_SET_MODE, "parameter": {"name": "normal"}},
            timeout=args.command_timeout,
        )
        if not args.skip_vui:
            await send_request(
                conn,
                label="vui_set_brightness",
                topic=RTC_TOPIC["VUI"],
                payload={
                    "api_id": VUI_SET_BRIGHTNESS,
                    "parameter": {"level": args.vui_brightness, "brightness": args.vui_brightness},
                },
                timeout=args.command_timeout,
            )
            vui_color_parameter: dict[str, Any] = {"color": args.vui_color, "time": args.vui_time}
            if args.vui_flash_cycle is not None:
                vui_color_parameter["flash_cycle"] = args.vui_flash_cycle
            await send_request(
                conn,
                label="vui_set_color",
                topic=RTC_TOPIC["VUI"],
                payload={"api_id": VUI_SET_COLOR, "parameter": vui_color_parameter},
                timeout=args.command_timeout,
            )
            if args.vui_hold > 0.0:
                print_event(
                    "VUI_HOLD",
                    {
                        "color": args.vui_color,
                        "time": args.vui_time,
                        "flash_cycle": args.vui_flash_cycle,
                        "hold_s": args.vui_hold,
                    },
                )
                await asyncio.sleep(args.vui_hold)

        action_results: list[dict[str, Any]] = []
        for command_name in args.posture_sequence:
            result = await send_request(
                conn,
                label=f"sport_{command_name}",
                topic=RTC_TOPIC["SPORT_MOD"],
                payload={"api_id": SPORT_CMD[command_name]},
                timeout=args.command_timeout,
            )
            action_results.append(result)
            await asyncio.sleep(args.posture_observe_s)
            print_event(
                "STATE_AFTER_ACTION", {"command": command_name, "frames": state_counts["frames"], "last": last_state}
            )

        send_zero_cmd_vel(conn, label="post_action_zero_cmd_vel")
        if action_results:
            latencies = [item["elapsed_ms"] for item in action_results]
            print_event(
                "ACTION_SUMMARY",
                {
                    "count": len(action_results),
                    "p50_ms": round(statistics.median(latencies), 2),
                    "max_ms": round(max(latencies), 2),
                    "ids_unique": len({item["id"] for item in action_results}) == len(action_results),
                    "mismatch_count": sum(
                        1
                        for item in action_results
                        if item["topic"] not in {"rt/api/sport/request", "rt/api/sport/response"}
                        or item["api_id"] not in SPORT_CMD.values()
                    ),
                },
            )
    finally:
        await disconnect_safely(conn, "primary")


async def run_multi_connect_validation(args: argparse.Namespace) -> None:
    conn1 = None
    conn2 = None
    try:
        try:
            conn1 = await connect(args.ip, "conn1")
            try:
                conn2 = await connect(args.ip, "conn2")
            except Exception as exc:  # noqa: BLE001 - second-connection behavior is firmware dependent and must be reported.
                print_event(
                    "MULTI_CONNECT_RESULT",
                    {
                        "conn1": "connected",
                        "conn2": "failed",
                        "conn2_error": str(exc),
                        "policy": "do_not_use_multi_control; use Jetson/Dora fan-out or explicit takeover",
                    },
                )
                return
        except Exception as exc:  # noqa: BLE001 - diagnostic script should produce machine-readable evidence.
            print_event("MULTI_CONNECT_RESULT", {"conn1": "failed", "conn1_error": str(exc)})
            return

        counts = {"conn1": 0, "conn2": 0}

        def make_callback(label: str):
            def callback(_message: dict[str, Any]) -> None:
                counts[label] += 1

            return callback

        conn1.datachannel.pub_sub.subscribe(RTC_TOPIC["LOW_STATE"], make_callback("conn1"))
        conn2.datachannel.pub_sub.subscribe(RTC_TOPIC["LOW_STATE"], make_callback("conn2"))
        await asyncio.sleep(args.state_wait)
        print_event("MULTI_CONNECT_OBSERVE", counts)

        send_zero_cmd_vel(conn1, label="conn1_zero_cmd_vel")
        send_zero_cmd_vel(conn2, label="conn2_zero_cmd_vel")
        await send_request(
            conn2,
            label="conn2_vui_set_color",
            topic=RTC_TOPIC["VUI"],
            payload={"api_id": VUI_SET_COLOR, "parameter": {"color": "purple", "time": 1}},
            timeout=args.command_timeout,
        )
        print_event(
            "MULTI_CONNECT_RESULT",
            {
                "conn1": "connected",
                "conn2": "connected",
                "control": "both_connections_accepted_safe_commands",
                "policy": "diagnostic_only; production control must stay single-owner",
            },
        )
    finally:
        await disconnect_safely(conn2, "conn2")
        await disconnect_safely(conn1, "conn1")


async def main() -> None:
    args = parse_args()
    if not args.only_multi_connect:
        await run_single_action_validation(args)
    if args.multi_connect:
        for attempt in range(1, args.multi_attempts + 1):
            print_event("MULTI_CONNECT_ATTEMPT", {"attempt": attempt, "total": args.multi_attempts})
            await run_multi_connect_validation(args)


if __name__ == "__main__":
    asyncio.run(main())
