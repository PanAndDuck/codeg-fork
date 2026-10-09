#!/usr/bin/env python3
"""Deterministic ACP v1 peer. stdout is exclusively newline JSON-RPC."""
import argparse
import json
import os
import shlex
import sys
import threading
import uuid


class Agent:
    def __init__(self, settle_ms=100):
        self.output_lock = threading.Lock()
        self.lock = threading.Lock()
        self.sessions = {}
        self.pending = {}
        self.request_seq = 0
        self.workers = []
        self.settle_ms = settle_ms

    def write(self, message):
        with self.output_lock:
            print(json.dumps({"jsonrpc": "2.0", **message}, separators=(",", ":")), flush=True)

    def reply(self, ident, result=None, error=None):
        self.write({"id": ident, **({"error": error} if error else {"result": result})})

    def update(self, sid, update_type, **fields):
        self.write({"method": "session/update", "params": {"sessionId": sid,
                    "update": {"sessionUpdate": update_type, **fields}}})

    def request(self, method, params, cancel):
        with self.lock:
            self.request_seq += 1
            ident = "mock-request-" + str(self.request_seq)
            event, result = threading.Event(), []
            self.pending[ident] = (event, result)
        self.write({"id": ident, "method": method, "params": params})
        try:
            while not event.wait(.05):
                if cancel.is_set():
                    return None
            if "error" in result[0]:
                raise ValueError("client rejected mock request: " + method)
            return result[0].get("result")
        finally:
            with self.lock:
                self.pending.pop(ident, None)

    def run_prompt(self, ident, params, cancel, turn):
        sid = params["sessionId"]
        try:
            text = "\n".join(b.get("text", "") for b in params.get("prompt", [])
                             if b.get("type") == "text")
            # codeg may prepend workspace context. Only explicit directive lines
            # drive scripted prompts; plain prompts are echoed when none exist.
            directives = [line for line in text.splitlines() if line.startswith("#")]
            if not directives:
                directives = ["#echo " + text]
            for directive in directives:
                if cancel.is_set():
                    break
                name, _, rest = directive.partition(" ")
                args = (dict(item.split("=", 1) for item in shlex.split(rest) if "=" in item)
                        if name in ("#stream", "#tools", "#plan") else {})
                if name in ("#echo", "#thought"):
                    self.update(sid, "agent_thought_chunk" if name == "#thought" else
                                "agent_message_chunk", content={"type": "text", "text": rest + "\n"})
                elif name == "#sleep":
                    cancel.wait(self.number(rest, 0, 3600, float))
                elif name == "#stream":
                    count = self.number(args.get("lines", "20"), 0, 100000)
                    interval = self.number(args.get("interval_ms", "50"), 0, 60000) / 1000
                    for i in range(1, count + 1):
                        if cancel.is_set():
                            break
                        self.update(sid, "agent_message_chunk", content={"type": "text",
                                    "text": f"line {i:06d}\n"})
                        if i < count:
                            cancel.wait(interval)
                elif name == "#tools":
                    count = self.number(args.get("count", "1"), 0, 10000)
                    size = self.number(args.get("output_kb", "1"), 0, 1024) * 1024
                    if count * size > 48 * 1024 * 1024:
                        raise ValueError("tool output budget exceeds 48 MiB")
                    interval = self.number(args.get("interval_ms", "0"), 0, 60000) / 1000
                    for i in range(1, count + 1):
                        if cancel.is_set():
                            break
                        tool = f"turn-{turn}-tool-{i:06d}"
                        self.update(sid, "tool_call", toolCallId=tool, title=f"Mock tool {i:06d}",
                                    kind="execute", status="in_progress", rawInput={"mock": i})
                        pattern = f"tool {i:06d} output\n"
                        output = (pattern * (size // len(pattern) + 1))[:size]
                        self.update(sid, "tool_call_update", toolCallId=tool, status="completed",
                                    content=[{"type": "content", "content": {"type": "text", "text": output}}])
                        cancel.wait(interval)
                elif name == "#plan":
                    count = self.number(args.get("steps", "3"), 1, 100)
                    self.update(sid, "plan", entries=[{"content": f"Mock step {i:03d}",
                                "priority": "medium", "status": "pending"} for i in range(1, count + 1)])
                elif name == "#permission":
                    self.request("session/request_permission", {"sessionId": sid,
                        "toolCall": {"toolCallId": f"permission-{turn}", "title": "Mock permission",
                                     "kind": "execute", "status": "pending", "rawInput": {"mock": True}},
                        "options": [{"optionId": "allow_once", "name": "Allow once", "kind": "allow_once"},
                                    {"optionId": "reject_once", "name": "Reject", "kind": "reject_once"}]}, cancel)
                elif name == "#question":
                    self.request("_x.ai/ask_user_question", {"sessionId": sid,
                        "toolCallId": f"question-{turn}", "questions": [{"question": "Mock choice?",
                        "options": [{"label": "Alpha", "description": "First deterministic option"},
                                    {"label": "Beta", "description": "Second deterministic option"}],
                        "multiSelect": False}]}, cancel)
                elif name == "#approve_plan":
                    self.request("_x.ai/exit_plan_mode", {"sessionId": sid,
                        "toolCallId": f"plan-{turn}", "planContent": "# Mock plan\n\n1. Verify recovery."}, cancel)
                elif name == "#fail":
                    raise ValueError("scripted mock failure")
                else:
                    raise ValueError("unknown directive: " + name)
            # codeg dispatches ACP notifications and prompt responses on
            # separate tasks. Keep this explicit and configurable: 0 reproduces
            # its late-final-delta race; ordinary recovery tests use 100 ms.
            cancel.wait(self.settle_ms / 1000)
            with self.lock:
                self.sessions[sid]["cancel"] = None
                self.reply(ident, {"stopReason": "cancelled" if cancel.is_set() else "end_turn"})
        except (ValueError, TypeError) as exc:
            with self.lock:
                self.sessions[sid]["cancel"] = None
                self.reply(ident, error={"code": -32000, "message": str(exc)})

    @staticmethod
    def number(raw, low, high, cast=int):
        value = cast(raw)
        if not low <= value <= high:
            raise ValueError(f"value outside {low}..{high}")
        return value

    def receive(self, msg):
        ident, method, params = msg.get("id"), msg.get("method"), msg.get("params", {})
        if method is None:
            with self.lock:
                pending = self.pending.get(ident)
                if pending:
                    pending[1].append(msg)
                    pending[0].set()
            return
        if method == "initialize":
            self.reply(ident, {"protocolVersion": 1, "agentCapabilities": {"loadSession": True},
                              "agentInfo": {"name": "codeg-e2e-mock", "version": "1.0.0"}, "authMethods": []})
        elif method in ("session/new", "session/load"):
            sid = params.get("sessionId") or "mock-" + uuid.uuid4().hex
            self.sessions.setdefault(sid, {"cancel": None, "turn": 0})
            self.reply(ident, {"sessionId": sid} if method == "session/new" else {})
        elif method == "session/prompt":
            sid = params.get("sessionId")
            with self.lock:
                session = self.sessions.get(sid)
                if not session:
                    self.reply(ident, error={"code": -32602, "message": "unknown session"})
                    return
                if session["cancel"] is not None:
                    self.reply(ident, error={"code": -32000, "message": "turn in progress"})
                    return
                cancel = threading.Event()
                session["cancel"] = cancel
                session["turn"] += 1
                worker = threading.Thread(target=self.run_prompt,
                    args=(ident, params, cancel, session["turn"]), daemon=True)
                self.workers.append(worker)
            worker.start()
        elif method == "session/cancel":
            with self.lock:
                session = self.sessions.get(params.get("sessionId"))
                if session and session["cancel"]:
                    session["cancel"].set()
        elif ident is not None:
            self.reply(ident, error={"code": -32601, "message": "method not found"})

    def run(self):
        try:
            for line in sys.stdin:
                try:
                    self.receive(json.loads(line))
                except (ValueError, TypeError, KeyError) as exc:
                    self.reply(None, error={"code": -32700, "message": str(exc)})
        finally:
            with self.lock:
                for session in self.sessions.values():
                    if session["cancel"]:
                        session["cancel"].set()
            for worker in self.workers:
                worker.join(timeout=2)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", action="version", version="codeg-e2e-mock 1.0.0")
    parser.add_argument("--settle-ms", type=int, default=100,
                        help="post-update wait before prompt result; 0 probes codeg's response/notification race")
    args = parser.parse_args()
    if not 0 <= args.settle_ms <= 10000:
        parser.error("--settle-ms must be 0–10000")
    try:
        Agent(args.settle_ms).run()
    except BrokenPipeError:
        os._exit(0)
