#!/usr/bin/env python3
"""Exercise content-aware faults and pending cards against the real relay."""
import argparse
import asyncio
import http.client
import json
import time

from faultctl import control
from harness import WebSocket, load_state, state_path
from register_mock import register
from smoke import Smoke


class FaultSmoke(Smoke):
    async def fault(self, command, **params):
        return await asyncio.to_thread(control, self.state, command, **params)

    async def snapshot(self):
        return await self.call("acp_get_session_snapshot", {"connectionId": self.session["connectionId"]})

    async def snapshot_until(self, predicate, timeout=10):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            snap = await self.snapshot()
            if predicate(snap):
                return snap
            await asyncio.sleep(.05)
        raise AssertionError("snapshot condition deadline")

    async def replace_socket(self, since=None):
        if self.ws:
            await self.ws.close()
        return await self.attach("proxy", since)

    def passed(self, name, detail):
        self.results[name] = detail
        print("PASS " + name + ": " + str(detail), flush=True)

    async def run(self):
        self.session = await asyncio.to_thread(register, self.state, "relay", True)
        try:
            await self.fault("reset")
            await self.snapshot_until(lambda s: s["status"] == "connected")
            await self.fault("blackhole_after_attach", count=1)
            self.ws = await WebSocket.connect(self.state["proxy_url"], self.state["token"], self.state["proxy_fingerprint"])
            ready = await self.ws.receive()
            assert ready.get("channel") == "__ready__", ready
            await self.ws.send({"action": "attach", "subscription_id": "blackhole",
                                "connection_id": self.session["connectionId"]})
            start = time.monotonic()
            try:
                await self.ws.receive(21)
                raise AssertionError("blackhole leaked a downstream message")
            except TimeoutError:
                pass
            assert (await self.snapshot())["status"] == "connected"
            self.passed("blackhole_after_attach", f"ready passed; downstream silent {time.monotonic() - start:.1f}s; HTTP readable")
            await self.fault("reset")
            await self.replace_socket()

            await self.fault("drop_attach", count=2)
            for _ in range(2):
                try:
                    await self.replace_socket()
                    raise AssertionError("drop_attach unexpectedly succeeded")
                except (EOFError, OSError, asyncio.IncompleteReadError):
                    pass
            assert (await self.replace_socket())["type"] == "snapshot"
            self.passed("drop_attach", "first two attaches closed before snapshot; third succeeded")

            await self.prompt("#stream lines=20 interval_ms=50")
            while len(self.lines) < 3:
                await self.next_event()
            await self.fault("inject_lagged")
            while True:
                msg = await self.ws.receive()
                if msg.get("type") == "event":
                    self.apply(msg["envelope"])
                if msg.get("type") == "detached":
                    assert msg["reason"] == "lagged"
                    break
            since = self.seq
            await self.ws.close()
            await asyncio.sleep(.15)
            assert (await self.replace_socket(since))["type"] == "replay"
            await self.until_complete()
            self.expect_lines(20)
            self.passed("inject_lagged", "detached + cursor replay; 20 ordered lines")

            for close_ws in (False, True):
                before = (await self.fault("status"))["counters"].get("prompt_requests", 0)
                await self.fault("drop_http_response", path="/api/acp_prompt", close_ws=close_ws)
                since = self.seq
                try:
                    await self.prompt("#stream lines=10 interval_ms=50", "proxy")
                    raise AssertionError("dropped response unexpectedly arrived")
                except (OSError, http.client.HTTPException):
                    pass
                if close_ws:
                    await asyncio.sleep(.15)
                    assert (await self.replace_socket(since))["type"] == "replay"
                await self.until_complete()
                self.expect_lines(10)
                status = await self.fault("status")
                assert status["counters"]["prompt_requests"] == before + 1
                assert status["counters"]["last_dropped_http_status"] == 200
                self.passed("lost_prompt_response_ws_" + str(close_ws), "one accepted POST, 10 lines, no resend")

            await self.fault("quiet_after_first_content", count=1)
            await self.prompt("#stream lines=10 interval_ms=50")
            while not self.lines:
                await self.next_event()
            await self.snapshot_until(lambda s: s["status"] == "connected" and s["live_message"] is None)
            try:
                while True:
                    msg = await self.ws.receive(.5)
                    assert msg.get("envelope", {}).get("type") != "turn_complete"
            except TimeoutError:
                pass
            self.passed("quiet_after_first_content", "first line visible; turn_complete suppressed; HTTP confirms finished")
            await self.fault("reset")
            await self.fault("omit_since_seq", count=1)
            first = await self.replace_socket(self.seq)
            assert first["type"] == "snapshot" and first["snapshot"]["status"] == "connected"
            assert (await self.fault("status"))["counters"]["omitted_since_seq"] >= 1
            self.passed("omit_since_seq", "finished-during-gap attach forced idle snapshot")

            await self.fault("block_ws", seconds=1)
            assert (await self.snapshot())["status"] == "connected"
            try:
                await self.replace_socket()
                raise AssertionError("blocked WS connected")
            except RuntimeError as exc:
                assert "503" in str(exc)
            await asyncio.sleep(1.05)
            await self.replace_socket()
            self.passed("block_ws", "WS upgrade rejected, HTTP unaffected, timed unblock")

            await self.fault("delay_attach", seconds=.3, count=1)
            start = time.monotonic()
            await self.replace_socket()
            assert time.monotonic() - start >= .3
            await self.fault("delay_http", path="/api/acp_get_session_snapshot", seconds=.3)
            start = time.monotonic()
            await self.call("acp_get_session_snapshot", {"connectionId": self.session["connectionId"]}, "proxy")
            assert time.monotonic() - start >= .3
            self.passed("delays", "attach and selected HTTP request delayed once")

            for directive, field in (("#permission", "pending_permission"), ("#question", "pending_question"),
                                     ("#approve_plan", "pending_plan_approval")):
                await self.prompt(directive + "\n#echo resolved")
                snap = await self.snapshot_until(lambda s: s.get(field))
                pending = snap[field]
                await self.fault("drop_ws")
                if field == "pending_permission":
                    await self.call("acp_respond_permission", {"connectionId": self.session["connectionId"],
                                    "requestId": pending["request_id"], "optionId": "allow_once"})
                elif field == "pending_question":
                    await self.call("acp_answer_question", {"connectionId": self.session["connectionId"],
                        "questionId": pending["question_id"], "answer": {"answers": [{
                            "questionId": pending["questions"][0]["id"], "labels": ["Alpha"]}]}})
                else:
                    await self.call("acp_answer_plan_approval", {"connectionId": self.session["connectionId"],
                        "approvalId": pending["approval_id"], "answer": {"decision": "approve", "feedback": ""}})
                await self.snapshot_until(lambda s: s["status"] == "connected")
                first = await self.replace_socket()
                assert not first["snapshot"].get(field)
                self.passed(directive, "pending in snapshot; resolved via second HTTP client during WS gap; card cleared")
        finally:
            await self.fault("reset")
            if self.ws:
                await self.ws.close()
            await self.call("acp_disconnect", {"connectionId": self.session["connectionId"]})
        target = state_path(self.state["name"]).with_suffix(".faults.json")
        target.write_text(json.dumps(self.results, indent=2) + "\n")
        print("PASS content-aware fault smoke; evidence=" + str(target), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", required=True)
    args = parser.parse_args()
    asyncio.run(FaultSmoke(load_state(args.name)).run())
