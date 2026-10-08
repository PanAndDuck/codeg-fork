#!/usr/bin/env python3
"""Prove relay certificate pinning and stream recovery through opaque TCP."""
import argparse
import asyncio
import json
from faultctl import control
from harness import load_state, state_path
from register_mock import register
from smoke import Smoke


async def run(state):
    assert state["proxy_mode"] == "passthrough"
    assert state["proxy_fingerprint"] == state["fingerprint"]
    s = Smoke(state)
    s.session = await asyncio.to_thread(register, state, "proxy", True)
    try:
        await s.attach("proxy")
        await s.prompt("#stream lines=50 interval_ms=50", "proxy")
        while len(s.lines) < 10:
            await s.next_event()
        since = s.seq
        await asyncio.to_thread(control, state, "drop_ws")
        await s.ws.close()
        await asyncio.sleep(.35)
        first = await s.attach("proxy", since)
        assert first["type"] == "replay"
        await s.until_complete()
        s.expect_lines(50)
        await asyncio.to_thread(control, state, "throttle", bps=65536)
        assert (await s.call("get_relay_status", through="proxy"))["state"] == "connected"
        await asyncio.to_thread(control, state, "reset")
        result = {"same_relay_pin": True, "lines": 50, "gaps": 0, "duplicates": 0,
                  "replayed_events": len(first["events"]), "throttled_https": "passed"}
        state_path(state["name"]).with_suffix(".passthrough.json").write_text(json.dumps(result, indent=2) + "\n")
        print("PASS passthrough: " + json.dumps(result), flush=True)
    finally:
        await asyncio.to_thread(control, state, "reset")
        if s.ws:
            await s.ws.close()
        await s.call("acp_disconnect", {"connectionId": s.session["connectionId"]})


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", required=True)
    args = parser.parse_args()
    asyncio.run(run(load_state(args.name)))
