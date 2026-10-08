"""Real stdio contract tests: a changed/missing emitted line must fail."""
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import unittest

AGENT = Path(__file__).resolve().parents[1] / "mock-agent/mock_acp_agent.py"


class MockTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(AGENT.exists(), "mock ACP executable must exist")
        self.p = subprocess.Popen([sys.executable, "-u", str(AGENT)],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE)
        self.addCleanup(self.stop)
        self.buffer = b""
        self.send(1, "initialize", {"protocolVersion": 1})
        self.assertEqual(self.read()["result"]["protocolVersion"], 1)
        self.send(2, "session/new", {"cwd": "/tmp", "mcpServers": []})
        self.sid = self.read()["result"]["sessionId"]

    def stop(self):
        self.p.stdin.close()
        self.p.wait(timeout=3)
        self.assertEqual(self.p.returncode, 0)
        self.p.stdout.close()
        self.p.stderr.close()

    def send(self, ident, method, params):
        msg = {"jsonrpc": "2.0", "method": method, "params": params}
        if ident is not None:
            msg["id"] = ident
        self.p.stdin.write((json.dumps(msg) + "\n").encode())
        self.p.stdin.flush()

    def read(self):
        while b"\n" not in self.buffer:
            ready, _, _ = select.select([self.p.stdout], [], [], 3)
            self.assertTrue(ready, "ACP message deadline exceeded")
            part = os.read(self.p.stdout.fileno(), 65536)
            self.assertTrue(part, "ACP exited before response")
            self.buffer += part
        line, self.buffer = self.buffer.split(b"\n", 1)
        return json.loads(line)

    def prompt(self, text):
        self.send(3, "session/prompt", {"sessionId": self.sid,
                  "prompt": [{"type": "text", "text": text}]})
        updates = []
        while True:
            msg = self.read()
            if msg.get("id") == 3:
                return updates, msg
            updates.append(msg["params"]["update"])

    def test_stream_in_order_and_plan_thought(self):
        updates, end = self.prompt("#plan steps=2\n#thought checking\n#stream lines=3 interval_ms=0")
        lines = [u["content"]["text"] for u in updates
                 if u["sessionUpdate"] == "agent_message_chunk"]
        self.assertEqual(lines, ["line 000001\n", "line 000002\n", "line 000003\n"])
        self.assertEqual(updates[0]["sessionUpdate"], "plan")
        self.assertTrue(any(u["sessionUpdate"] == "agent_thought_chunk" for u in updates))
        self.assertEqual(end["result"]["stopReason"], "end_turn")

    def test_tools_and_fail(self):
        updates, end = self.prompt("#tools count=2 output_kb=8")
        done = [u for u in updates if u["sessionUpdate"] == "tool_call_update"]
        self.assertEqual(len(done), 2)
        self.assertEqual(len(done[0]["content"][0]["content"]["text"].encode()), 8192)
        self.assertEqual(end["result"]["stopReason"], "end_turn")
        _, end = self.prompt("#fail")
        self.assertEqual(end["error"]["code"], -32000)

    def test_echo_is_verbatim_not_shell_syntax(self):
        updates, end = self.prompt("#echo don't parse this = 'literal")
        self.assertEqual(end.get("result", {}).get("stopReason"), "end_turn")
        self.assertEqual(updates[0]["content"]["text"], "don't parse this = 'literal\n")

    def test_cancel_while_sleeping_and_load(self):
        self.send(3, "session/prompt", {"sessionId": self.sid,
                  "prompt": [{"type": "text", "text": "#echo started\n#sleep 30"}]})
        self.assertEqual(self.read()["params"]["update"]["content"]["text"], "started\n")
        self.send(None, "session/cancel", {"sessionId": self.sid})
        self.assertEqual(self.read()["result"]["stopReason"], "cancelled")
        self.send(4, "session/load", {"sessionId": self.sid, "cwd": "/tmp", "mcpServers": []})
        self.assertEqual(self.read()["result"], {})

    def test_permission_waits_for_response(self):
        self.send(3, "session/prompt", {"sessionId": self.sid,
                  "prompt": [{"type": "text", "text": "#permission\n#echo approved"}]})
        req = self.read()
        self.assertEqual(req["method"], "session/request_permission")
        self.p.stdin.write((json.dumps({"jsonrpc": "2.0", "id": req["id"],
            "result": {"outcome": {"outcome": "selected", "optionId": "allow_once"}}}) + "\n").encode())
        self.p.stdin.flush()
        self.assertEqual(self.read()["params"]["update"]["content"]["text"], "approved\n")
        self.assertEqual(self.read()["result"]["stopReason"], "end_turn")


if __name__ == "__main__":
    unittest.main()
