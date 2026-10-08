#!/usr/bin/env python3
"""Pinned API and WS inspection against a named disposable environment."""
import argparse
import asyncio
import json
import time
from harness import WebSocket, api, endpoint, load_state


async def watch(state, args):
    url, pin = endpoint(state, args.through)
    ws = await WebSocket.connect(url, state["token"], pin)
    try:
        first = await ws.attach(args.connection, args.since_seq)
        print(json.dumps(first), flush=True)
        end = time.monotonic() + args.seconds
        while time.monotonic() < end:
            try:
                msg = await ws.receive(min(30, end - time.monotonic()))
            except TimeoutError:
                continue
            if msg.get("type") in ("event", "detached", "snapshot", "replay"):
                print(json.dumps(msg), flush=True)
    finally:
        await ws.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", required=True)
    parser.add_argument("--through", choices=("relay", "proxy", "server"), default="relay")
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--api")
    group.add_argument("--watch", action="store_true")
    parser.add_argument("--json", default="{}")
    parser.add_argument("--bytes", action="store_true", help="print response byte count instead of JSON")
    parser.add_argument("--connection")
    parser.add_argument("--since-seq", type=int)
    parser.add_argument("--seconds", type=float, default=60)
    args = parser.parse_args()
    state = load_state(args.name)
    if args.watch:
        if not args.connection:
            parser.error("--watch requires --connection")
        asyncio.run(watch(state, args))
    else:
        url, pin = endpoint(state, args.through)
        result = api(url, state["token"], args.api, json.loads(args.json), pin, raw=args.bytes)
        print(len(result) if args.bytes else json.dumps(result, indent=2))
