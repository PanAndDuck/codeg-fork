#!/usr/bin/env python3
"""Real relay smoke: ordered streams, large live snapshot and cursor recovery."""
import argparse
import asyncio
import json
import time
import uuid

from faultctl import control
from harness import WebSocket, api, endpoint, load_state, state_path
from register_mock import register


class Smoke:
    def __init__(self, state):
        self.state = state
        self.session = None
        self.ws = None
        self.seq = 0
        self.lines = []
        self.events = []
        self.results = {}

    async def call(self, command, params=None, through="relay", raw=False):
        url, pin = endpoint(self.state, through)
        return await asyncio.to_thread(api, url, self.state["token"], command, params, pin, raw)

    async def attach(self, through="relay", since=None):
        url, pin = endpoint(self.state, through)
        self.ws = await WebSocket.connect(url, self.state["token"], pin)
        first = await self.ws.attach(self.session["connectionId"], since)
        if first["type"] == "snapshot":
            self.seq = first["event_seq"]
        elif first["type"] == "replay":
            for event in first["events"]:
                self.apply(event)
        else:
            raise AssertionError(f"attach returned {first['type']}")
        return first

    def apply(self, event):
        assert event["seq"] == self.seq + 1, f"seq gap/duplicate: {self.seq} → {event['seq']}"
        self.seq = event["seq"]
        self.events.append(event["type"])
        if event["type"] == "content_delta":
            self.lines.extend(event["text"].splitlines())

    async def next_event(self, timeout=30):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            msg = await self.ws.receive(max(.1, deadline - time.monotonic()))
            if msg.get("type") == "event":
                self.apply(msg["envelope"])
                return msg["envelope"]
            if msg.get("type") == "detached":
                raise AssertionError("unexpected detach: " + msg["reason"])
        raise TimeoutError("event deadline")

    async def until_complete(self):
        while (await self.next_event())["type"] != "turn_complete":
            pass

    async def prompt(self, text, through="relay"):
        self.lines = []
        self.events = []
        await self.call("acp_prompt", {"connectionId": self.session["connectionId"],
            "conversationId": self.session["conversationId"], "folderId": self.session["folderId"],
            "clientMessageId": "e2e-" + uuid.uuid4().hex,
            "blocks": [{"type": "text", "text": text}]}, through)

    def expect_lines(self, count):
        assert self.lines == [f"line {i:06d}" for i in range(1, count + 1)], self.lines

    async def run(self):
        assert self.state["proxy_mode"] == "terminate", "core smoke requires terminate proxy"
        self.session = await asyncio.to_thread(register, self.state, "relay", True)
        print("registered + enabled via relay; " + json.dumps(self.session), flush=True)
        try:
            await self.attach()
            await self.prompt("#stream lines=50 interval_ms=50")
            await self.until_complete()
            self.expect_lines(50)
            self.results["stream"] = {"lines": 50, "ordered": True, "turn_complete": True}
            print("PASS relay stream: 50/50 ordered lines, no duplicates, turn_complete", flush=True)

            # TurnComplete intentionally clears server live state. Park the
            # scripted turn for five seconds so a live snapshot is observable.
            await self.prompt("#tools count=250 output_kb=8\n#sleep 5")
            consumer = asyncio.create_task(self.until_complete())
            try:
                deadline = time.monotonic() + 30
                while time.monotonic() < deadline:
                    data = await self.call("acp_get_session_snapshot", {"connectionId": self.session["connectionId"]}, raw=True)
                    snapshot = json.loads(data)
                    if len(snapshot["active_tool_calls"]) == 250 and len(data) > 1024 * 1024:
                        break
                    await asyncio.sleep(.1)
                else:
                    raise AssertionError("250-tool live snapshot never exceeded 1 MiB")
                self.results["snapshot_bytes"] = len(data)
                # Also prove a >1 MiB WS attach passes through the terminating proxy.
                large_ws = await WebSocket.connect(self.state["proxy_url"], self.state["token"], self.state["proxy_fingerprint"])
                try:
                    large = await large_ws.attach(self.session["connectionId"])
                    assert large["type"] == "snapshot"
                    assert len(json.dumps(large).encode()) > 1024 * 1024
                    self.results["large_ws_snapshot_bytes"] = len(json.dumps(large).encode())
                finally:
                    await large_ws.close()
                print(f"PASS live snapshot: HTTP {len(data)} bytes; WS {self.results['large_ws_snapshot_bytes']} bytes", flush=True)
                await consumer
            finally:
                if not consumer.done():
                    consumer.cancel()
                    await asyncio.gather(consumer, return_exceptions=True)

            await self.ws.close()
            await self.attach("proxy")
            await self.prompt("#stream lines=50 interval_ms=50", "proxy")
            while len(self.lines) < 10:
                await self.next_event()
            since = self.seq
            await asyncio.to_thread(control, self.state, "drop_ws")
            await self.ws.close()
            await asyncio.sleep(.35)
            first = await self.attach("proxy", since)
            assert first["type"] == "replay", f"expected small-gap replay, got {first['type']}"
            replayed = len(first["events"])
            await self.until_complete()
            self.expect_lines(50)
            self.results["recovery"] = {"since_seq": since, "replayed_events": replayed,
                                        "lines": 50, "gaps": 0, "duplicates": 0}
            print(f"PASS drop_ws + since_seq={since}: {replayed} replay events; 50/50 lines, gaps=0 duplicates=0", flush=True)
        finally:
            if self.ws:
                await self.ws.close()
            if self.session:
                await self.call("acp_disconnect", {"connectionId": self.session["connectionId"]})
        target = state_path(self.state["name"]).with_suffix(".smoke.json")
        target.write_text(json.dumps(self.results, indent=2) + "\n")
        print("PASS core smoke; evidence=" + str(target), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", required=True)
    args = parser.parse_args()
    asyncio.run(Smoke(load_state(args.name)).run())
