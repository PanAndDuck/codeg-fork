#!/usr/bin/env python3
"""Local asyncio fault proxy. TLS pass-through or separately pinned termination."""
import argparse
import asyncio
from collections import Counter
import json
from pathlib import Path
import signal
import ssl
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from harness import MAX_FRAME, check_pin, frame_bytes, read_frame, tls_context

HEAD_LIMIT = 65536
BODY_LIMIT = 16 * 1024 * 1024


async def read_head(reader):
    data = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 15)
    if len(data) > HEAD_LIMIT:
        raise ValueError("HTTP head exceeds 64 KiB")
    lines = data[:-4].split(b"\r\n")
    fields = []
    for line in lines[1:]:
        key, value = line.split(b":", 1)
        fields.append((key.lower(), value.strip()))
    return lines[0], fields


def get_header(fields, key, default=b""):
    return next((v for k, v in fields if k == key), default)


def head_bytes(first, fields):
    return first + b"\r\n" + b"".join(k + b": " + v + b"\r\n" for k, v in fields) + b"\r\n"


async def request_body(reader, fields):
    if get_header(fields, b"transfer-encoding"):
        if get_header(fields, b"transfer-encoding").lower() != b"chunked":
            raise ValueError("unsupported HTTP transfer coding")
        data = bytearray()
        while True:
            line = await reader.readuntil(b"\r\n")
            size = int(line.split(b";", 1)[0].strip(), 16)
            if len(data) + size > BODY_LIMIT:
                raise ValueError("HTTP request exceeds 16 MiB")
            if size == 0:
                while await reader.readuntil(b"\r\n") != b"\r\n":
                    pass
                return bytes(data)
            data.extend(await reader.readexactly(size))
            if await reader.readexactly(2) != b"\r\n":
                raise ValueError("invalid HTTP chunk")
    size = int(get_header(fields, b"content-length", b"0"))
    if not 0 <= size <= BODY_LIMIT:
        raise ValueError("HTTP request exceeds 16 MiB")
    return await reader.readexactly(size)


class Connection:
    def __init__(self, writer):
        self.client = writer
        self.upstream = None
        self.ws = False
        self.attached = False
        self.subscription = None
        self.connection_id = None
        self.blackhole = False
        self.quiet = False
        self.quiet_stopped = False
        self.detached = False
        self.write_lock = asyncio.Lock()

    def abort(self):
        for writer in (self.client, self.upstream):
            if writer:
                writer.transport.abort()


class Proxy:
    CONTENT_COMMANDS = {"blackhole_after_attach", "omit_since_seq", "inject_lagged", "drop_http_response",
                        "quiet_after_first_content", "drop_attach", "delay_attach", "delay_http"}

    def __init__(self, mode, upstream_port, fingerprint):
        self.mode, self.upstream_port, self.fingerprint = mode, upstream_port, fingerprint
        self.connections = set()
        self.tasks = set()
        self.counters = Counter()
        self.throttle_bps = 0
        self.block_until = 0
        self.block_all_until = 0
        self.faults = {}

    def take(self, name):
        value = self.faults.get(name)
        if not value:
            return None
        if value["count"] > 0:
            value["count"] -= 1
            if value["count"] == 0:
                self.faults.pop(name)
        return value

    def status(self):
        return {"mode": self.mode, "connections": len(self.connections),
                "websockets": sum(c.ws for c in self.connections), "throttle_bps": self.throttle_bps,
                "block_ws_remaining": max(0, round(self.block_until - time.monotonic(), 2)),
                "block_all_remaining": max(0, round(self.block_all_until - time.monotonic(), 2)),
                "faults": self.faults, "counters": dict(self.counters),
                "passthrough_scope": "all TCP connections; TLS hides HTTP/WS classification" if self.mode == "passthrough" else None}

    async def control(self, msg):
        name = msg.get("command")
        if name in self.CONTENT_COMMANDS and self.mode != "terminate":
            raise ValueError("content-aware faults require terminate mode")
        if name == "reset":
            self.faults.clear()
            self.throttle_bps = 0
            self.block_until = self.block_all_until = 0
            for c in self.connections:
                c.blackhole = c.quiet = c.quiet_stopped = c.detached = False
        elif name in ("drop_ws", "drop_all", "block_ws", "block_all"):
            seconds = float(msg.get("seconds", 30))
            if not 0 <= seconds <= 3600:
                raise ValueError("seconds must be 0–3600")
            if name == "block_ws":
                self.block_until = time.monotonic() + seconds
            if name == "block_all":
                self.block_all_until = time.monotonic() + seconds
            affected = 0
            for c in list(self.connections):
                if self.mode == "passthrough" or name in ("drop_all", "block_all") or c.ws:
                    c.abort()
                    affected += 1
            self.counters["dropped_connections"] += affected
        elif name == "throttle":
            bps = int(msg.get("bps", 0))
            if not 0 <= bps <= 1024 * 1024 * 1024:
                raise ValueError("bps must be 0–1 GiB/s")
            self.throttle_bps = bps
        elif name == "inject_lagged":
            for c in list(self.connections):
                if c.ws and c.subscription and not c.detached:
                    await self.send(c, c.client, frame_bytes(1, json.dumps({"type": "detached",
                        "subscription_id": c.subscription, "reason": "lagged"}).encode()), downstream=True)
                    c.detached = True
                    self.counters["injected_lagged"] += 1
        elif name in self.CONTENT_COMMANDS:
            count = int(msg.get("count", 1 if name in ("omit_since_seq", "drop_http_response", "delay_http") else -1))
            if count != -1 and not 1 <= count <= 10000:
                raise ValueError("count must be -1 (until reset) or 1–10000")
            fault = {"count": count}
            if name in ("drop_http_response", "delay_http"):
                path = msg.get("path", "/api/acp_prompt")
                if not isinstance(path, str) or not path.startswith("/api/") or "?" in path:
                    raise ValueError("fault path must be /api/<command> without query")
                fault.update(path=path, close_ws=bool(msg.get("close_ws", False)))
            if name in ("delay_http", "delay_attach"):
                seconds = float(msg.get("seconds", 8))
                if not 0 <= seconds <= 60:
                    raise ValueError("delay seconds must be 0–60")
                fault["seconds"] = seconds
            self.faults[name] = fault
            if name == "quiet_after_first_content":
                for c in self.connections:
                    if c.ws:
                        c.quiet = True
                        c.quiet_stopped = False
        elif name != "status":
            raise ValueError("unknown command")
        return self.status()

    async def send(self, c, writer, data, downstream=False):
        async def write():
            offset = 0
            while offset < len(data):
                bps = self.throttle_bps
                count = min(16384, max(1, bps // 10)) if bps else 65536
                chunk = data[offset:offset + count]
                writer.write(chunk)
                await writer.drain()
                offset += len(chunk)
                if bps:
                    await asyncio.sleep(len(chunk) / bps)
        if downstream:
            async with c.write_lock:
                await write()
        else:
            await write()

    async def dial(self):
        reader, writer = await asyncio.wait_for(asyncio.open_connection("127.0.0.1", self.upstream_port,
            ssl=tls_context() if self.mode == "terminate" else None,
            server_hostname="localhost" if self.mode == "terminate" else None), 10)
        try:
            if self.mode == "terminate":
                check_pin(writer.get_extra_info("ssl_object"), self.fingerprint)
            return reader, writer
        except BaseException:
            writer.close()
            await writer.wait_closed()
            raise

    async def pair(self, *coros):
        tasks = [asyncio.create_task(coro) for coro in coros]
        try:
            done, _ = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
            for task in done:
                task.result()
        finally:
            for task in tasks:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)

    async def raw_pipe(self, reader, writer, c, downstream):
        while data := await reader.read(65536):
            await self.send(c, writer, data, downstream)

    async def transform(self, c, payload, downstream):
        try:
            msg = json.loads(payload)
        except (ValueError, UnicodeDecodeError):
            return payload
        if not isinstance(msg, dict):
            return payload
        if not downstream and msg.get("action") == "attach":
            c.attached = True
            c.subscription = msg.get("subscription_id")
            c.connection_id = msg.get("connection_id")
            c.detached = False
            self.counters["attaches"] += 1
            if self.take("drop_attach"):
                self.counters["dropped_attaches"] += 1
                c.abort()
                return None
            if self.take("omit_since_seq"):
                msg.pop("since_seq", None)
                self.counters["omitted_since_seq"] += 1
                payload = json.dumps(msg).encode()
            if self.take("blackhole_after_attach"):
                c.blackhole = True
            if self.take("quiet_after_first_content"):
                c.quiet = True
            delay = self.take("delay_attach")
            if delay:
                await asyncio.sleep(delay["seconds"])
        if downstream:
            if c.quiet_stopped:
                self.counters["quiet_dropped_messages"] += 1
                return None
            channel = msg.get("channel")
            envelope = msg.get("envelope")
            if c.detached and (msg.get("subscription_id") == c.subscription or
                               isinstance(channel, str) and channel.startswith("acp:")):
                return None
            if (c.quiet and msg.get("type") == "event" and isinstance(envelope, dict)
                    and envelope.get("type") in ("content_delta", "tool_call", "tool_call_update")):
                c.quiet_stopped = True
                self.counters["quiet_started"] += 1
        return payload

    async def ws_pipe(self, reader, writer, c, downstream):
        fragment = bytearray()
        fragment_opcode = None
        while True:
            fin, opcode, payload = await read_frame(reader, expect_masked=not downstream)
            if downstream and c.blackhole and c.attached:
                self.counters["blackholed_frames"] += 1
                continue
            if opcode in (1, 2):
                if fragment_opcode is not None:
                    raise ValueError("new WS message before prior fragments completed")
                fragment_opcode = opcode
            if opcode in (0, 1, 2):
                if fragment_opcode is None:
                    raise ValueError("WS continuation without a message")
                fragment.extend(payload)
                if len(fragment) > MAX_FRAME:
                    raise ValueError("WS message exceeds 64 MiB")
                if not fin:
                    continue
                opcode = fragment_opcode
                payload = (await self.transform(c, bytes(fragment), downstream)
                           if opcode == 1 else bytes(fragment))
                fragment.clear()
                fragment_opcode = None
                if payload is None:
                    continue
            await self.send(c, writer, frame_bytes(opcode, payload, masked=not downstream, fin=fin), downstream)
            if opcode == 8:
                return

    async def terminated(self, reader, c):
        first, fields = await read_head(reader)
        method, target, _ = first.decode("ascii").split(" ", 2)
        path = target.split("?", 1)[0]
        is_ws = get_header(fields, b"upgrade").lower() == b"websocket"
        if is_ws and time.monotonic() < self.block_until:
            self.counters["blocked_ws"] += 1
            await self.send(c, c.client, b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", True)
            return
        self.counters["http_requests"] += 1
        if path == "/api/acp_prompt":
            self.counters["prompt_requests"] += 1
        if path == "/api/acp_touch_connection":
            self.counters["touch_requests"] += 1
            self.counters["last_touch_unix"] = int(time.time())
        delay = self.faults.get("delay_http")
        if delay and delay["path"] == path:
            await asyncio.sleep(self.take("delay_http")["seconds"])
        body = await asyncio.wait_for(request_body(reader, fields), 30)
        upstream_reader, c.upstream = await self.dial()
        # HTTP/1.1 only, one non-WS request per downstream connection. Removing
        # compression negotiation keeps WS JSON visible and response drop exact.
        fields = [(k, v) for k, v in fields if k not in
                  (b"connection", b"content-length", b"transfer-encoding", b"sec-websocket-extensions", b"expect")]
        fields += [(b"connection", b"Upgrade" if is_ws else b"close"), (b"content-length", str(len(body)).encode())]
        await self.send(c, c.upstream, head_bytes(first, fields) + body)
        response, response_fields = await read_head(upstream_reader)
        if is_ws and response.split(b" ")[1] == b"101":
            c.ws = True
            self.counters["upgraded_ws"] += 1
            await self.send(c, c.client, head_bytes(response, response_fields), True)
            await self.pair(self.ws_pipe(reader, c.upstream, c, False), self.ws_pipe(upstream_reader, c.client, c, True))
            return
        drop = self.faults.get("drop_http_response")
        drop = self.take("drop_http_response") if method == "POST" and drop and drop["path"] == path else None
        if not drop:
            response_fields = [(k, v) for k, v in response_fields if k != b"connection"] + [(b"connection", b"close")]
            await self.send(c, c.client, head_bytes(response, response_fields), True)
        # Upstream was explicitly asked to close; stream unbounded response
        # bodies without retaining the >1 MiB snapshot in the proxy's memory.
        while data := await upstream_reader.read(65536):
            if not drop:
                await self.send(c, c.client, data, True)
        if drop:
            self.counters["dropped_http_responses"] += 1
            self.counters["last_dropped_http_status"] = int(response.split(b" ")[1])
            if drop["close_ws"]:
                for other in list(self.connections):
                    if other.ws:
                        other.abort()
            c.abort()

    async def accept(self, reader, writer):
        task = asyncio.current_task()
        self.tasks.add(task)
        c = Connection(writer)
        self.connections.add(c)
        self.counters["accepted_connections"] += 1
        try:
            if len(self.connections) > 128:
                return
            if time.monotonic() < self.block_all_until:
                return
            if self.mode == "passthrough":
                if time.monotonic() < self.block_until:
                    return
                upstream, c.upstream = await self.dial()
                await self.pair(self.raw_pipe(reader, c.upstream, c, False), self.raw_pipe(upstream, writer, c, True))
            else:
                await self.terminated(reader, c)
        except (OSError, ValueError, asyncio.IncompleteReadError, asyncio.LimitOverrunError, TimeoutError):
            self.counters["transport_ends"] += 1
        finally:
            c.abort()
            self.connections.discard(c)
            self.tasks.discard(task)

    async def control_http(self, reader, writer):
        try:
            first, fields = await read_head(reader)
            method, path, _ = first.decode().split(" ", 2)
            if path == "/status":
                result = self.status()
            elif path == "/control" and method == "POST":
                data = await asyncio.wait_for(request_body(reader, fields), 5)
                if len(data) > 8192:
                    raise ValueError("control JSON too large")
                result = await self.control(json.loads(data))
            else:
                raise ValueError("use GET /status or POST /control")
            status = b"200 OK"
        except (OSError, ValueError, asyncio.IncompleteReadError, asyncio.LimitOverrunError, TimeoutError) as exc:
            status, result = b"400 Bad Request", {"error": str(exc)}
        try:
            body = json.dumps(result).encode()
            writer.write(b"HTTP/1.1 " + status + b"\r\nContent-Type: application/json\r\nContent-Length: " +
                         str(len(body)).encode() + b"\r\nConnection: close\r\n\r\n" + body)
            await writer.drain()
        except OSError:
            pass
        finally:
            writer.close()
            try:
                await writer.wait_closed()
            except OSError:
                pass

    async def close(self):
        for c in list(self.connections):
            c.abort()
        for task in list(self.tasks):
            task.cancel()
        await asyncio.gather(*list(self.tasks), return_exceptions=True)


async def main(args):
    context = None
    if args.mode == "terminate":
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(args.cert, args.key)
        context.set_alpn_protocols(["http/1.1"])
    proxy = Proxy(args.mode, args.upstream_port, args.upstream_fingerprint)
    listener = await asyncio.start_server(proxy.accept, "127.0.0.1", args.port, ssl=context, limit=HEAD_LIMIT)
    control = await asyncio.start_server(proxy.control_http, "127.0.0.1", args.control_port, limit=HEAD_LIMIT)
    event = asyncio.Event()
    for sig in (signal.SIGINT, signal.SIGTERM):
        asyncio.get_running_loop().add_signal_handler(sig, event.set)
    print(json.dumps({"mode": args.mode, "port": args.port, "control_port": args.control_port}), flush=True)
    async with listener, control:
        await event.wait()
    await proxy.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("passthrough", "terminate"), required=True)
    parser.add_argument("--port", type=int, default=18444)
    parser.add_argument("--control-port", type=int, default=18445)
    parser.add_argument("--upstream-port", type=int, default=18443)
    parser.add_argument("--upstream-fingerprint", required=True)
    parser.add_argument("--cert")
    parser.add_argument("--key")
    args = parser.parse_args()
    if not all(18400 <= p <= 18499 for p in (args.port, args.upstream_port)) or not 18445 <= args.control_port <= 18449:
        parser.error("data ports must be 18400–18499; control 18445–18449")
    if len({args.port, args.control_port, args.upstream_port}) != 3:
        parser.error("ports must be distinct")
    if args.mode == "terminate" and (not args.cert or not args.key):
        parser.error("termination requires --cert and --key")
    asyncio.run(main(args))
