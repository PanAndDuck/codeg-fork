#!/usr/bin/env python3
"""Control only the fault proxy recorded in a named harness environment."""
import argparse
import json
from harness import api, load_state


def control(state, command, **params):
    return api(f"http://127.0.0.1:{state['control_port']}", "", "/control", {"command": command, **params})


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", required=True)
    parser.add_argument("command", choices=("status", "reset", "drop_ws", "drop_all", "block_ws", "block_all",
        "throttle", "blackhole_after_attach", "omit_since_seq", "inject_lagged", "drop_http_response",
        "quiet_after_first_content", "drop_attach", "delay_attach", "delay_http"))
    parser.add_argument("--seconds", type=float)
    parser.add_argument("--bps", type=int)
    parser.add_argument("--count", type=int)
    parser.add_argument("--path")
    parser.add_argument("--close-ws", action="store_true", default=None)
    args = vars(parser.parse_args())
    state = load_state(args.pop("name"))
    command = args.pop("command")
    print(json.dumps(control(state, command, **{k: v for k, v in args.items() if v is not None}), indent=2))
