#!/usr/bin/env python3
"""Diagnose codeg's final ACP delta vs turn_complete race without hiding it."""
import argparse
import asyncio
import json
from harness import load_state, state_path
from register_mock import register
from smoke import Smoke


async def run(args):
    smoke = Smoke(load_state(args.name))
    smoke.session = await asyncio.to_thread(register, smoke.state, "relay", True, args.settle_ms)
    results = []
    try:
        await smoke.attach()
        for i in range(args.turns):
            await smoke.prompt("#stream lines=20 interval_ms=50")
            await smoke.until_complete()
            result = {"turn": i + 1, "lines_at_complete": len(smoke.lines),
                      "complete_seq": smoke.seq, "late_content": []}
            try:
                while True:
                    msg = await smoke.ws.receive(.3)
                    if msg.get("type") == "event":
                        event = msg["envelope"]
                        smoke.apply(event)
                        if event["type"] == "content_delta":
                            result["late_content"].append({"seq": event["seq"], "text": event["text"]})
            except TimeoutError:
                pass
            results.append(result)
            print(json.dumps(result), flush=True)
    finally:
        await smoke.ws.close()
        await smoke.call("acp_disconnect", {"connectionId": smoke.session["connectionId"]})
    target = state_path(args.name).with_suffix(f".completion-{args.settle_ms}.json")
    target.write_text(json.dumps(results, indent=2) + "\n")
    late = sum(bool(r["late_content"]) or r["lines_at_complete"] != 20 for r in results)
    print(f"completion-order deviations: {late}/{len(results)}; settle_ms={args.settle_ms}")
    return bool(late)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", required=True)
    parser.add_argument("--settle-ms", type=int, default=0)
    parser.add_argument("--turns", type=int, default=5)
    args = parser.parse_args()
    raise SystemExit(1 if asyncio.run(run(args)) else 0)
