"""Shared local-only state, pinned HTTPS and RFC6455 test client utilities."""
import asyncio
import base64
import hashlib
import http.client
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import ssl
import struct
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parent
STATE = ROOT / ".state"
MAX_FRAME = 64 * 1024 * 1024


def state_path(name):
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,47}", name):
        raise ValueError("name must be 1–48 letters/digits/_/-")
    return STATE / (name + ".json")


def load_state(name):
    state = json.loads(state_path(name).read_text())
    if state["repo"] != str(ROOT.parent):
        raise ValueError("state belongs to a different worktree")
    return state


def local_url(url):
    parsed = urlsplit(url)
    host = parsed.hostname
    if host != "localhost" and not ipaddress.ip_address(host).is_loopback:
        raise ValueError("harness only calls loopback endpoints")
    port = parsed.port
    if not (port and (3090 <= port <= 3099 or 18400 <= port <= 18499)):
        raise ValueError("harness endpoint must use a reserved test port")
    if parsed.scheme not in ("http", "https", "ws", "wss") or parsed.username or parsed.query:
        raise ValueError("invalid local test URL")
    return parsed


def tls_context():
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    context.set_alpn_protocols(["http/1.1"])
    return context


def check_pin(tls, fingerprint):
    actual = hashlib.sha256(tls.getpeercert(binary_form=True)).hexdigest()
    if not fingerprint or not secrets.compare_digest(actual, fingerprint):
        raise ssl.SSLError("test endpoint certificate fingerprint mismatch")


def api(url, token, command, params=None, fingerprint=None, raw=False, timeout=75):
    parsed = local_url(url)
    if parsed.scheme == "https":
        conn = http.client.HTTPSConnection(parsed.hostname, parsed.port,
                                            timeout=timeout, context=tls_context())
    elif parsed.scheme == "http":
        conn = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=timeout)
    else:
        raise ValueError("HTTP API requires http/https")
    try:
        conn.connect()
        if parsed.scheme == "https":
            check_pin(conn.sock, fingerprint)
        path = command if command.startswith("/") else "/api/" + command
        conn.request("POST", parsed.path.rstrip("/") + path,
                     json.dumps(params or {}).encode(), {"Authorization": "Bearer " + token,
                     "Content-Type": "application/json", "Connection": "close"})
        response = conn.getresponse()
        data = response.read()
        if response.status >= 400:
            raise RuntimeError(f"{command}: HTTP {response.status}: {data[:512].decode(errors='replace')}")
        return data if raw else json.loads(data)
    finally:
        conn.close()


def frame_bytes(opcode, payload=b"", masked=False, fin=True):
    head = bytes([(0x80 if fin else 0) | opcode])
    bit = 0x80 if masked else 0
    n = len(payload)
    if n < 126:
        head += bytes([bit | n])
    elif n <= 65535:
        head += bytes([bit | 126]) + struct.pack("!H", n)
    else:
        head += bytes([bit | 127]) + struct.pack("!Q", n)
    if masked:
        key = secrets.token_bytes(4)
        return head + key + bytes(b ^ key[i % 4] for i, b in enumerate(payload))
    return head + payload


async def read_frame(reader, expect_masked=None):
    header = await reader.readexactly(2)
    first, second = header
    if first & 0x70:
        raise ValueError("compressed/reserved WS bits unsupported")
    opcode = first & 15
    if opcode not in (0, 1, 2, 8, 9, 10):
        raise ValueError("invalid WS opcode")
    if expect_masked is not None and bool(second & 128) != expect_masked:
        raise ValueError("incorrect WS masking direction")
    size = second & 127
    if size == 126:
        size = struct.unpack("!H", await reader.readexactly(2))[0]
    elif size == 127:
        size = struct.unpack("!Q", await reader.readexactly(8))[0]
    if size > MAX_FRAME:
        raise ValueError("WS frame exceeds 64 MiB")
    if opcode >= 8 and (not first & 128 or size > 125):
        raise ValueError("invalid WS control frame")
    key = await reader.readexactly(4) if second & 128 else None
    data = await reader.readexactly(size)
    if key:
        data = bytes(b ^ key[i % 4] for i, b in enumerate(data))
    return bool(first & 128), first & 15, data


class WebSocket:
    def __init__(self, reader, writer):
        self.reader, self.writer = reader, writer
        self.fragment = bytearray()

    @classmethod
    async def connect(cls, url, token, fingerprint):
        parsed = local_url(url)
        secure = parsed.scheme in ("https", "wss")
        reader, writer = await asyncio.wait_for(asyncio.open_connection(parsed.hostname, parsed.port,
            ssl=tls_context() if secure else None, server_hostname=parsed.hostname if secure else None), 10)
        try:
            if secure:
                check_pin(writer.get_extra_info("ssl_object"), fingerprint)
            key = base64.b64encode(secrets.token_bytes(16)).decode()
            proto = base64.urlsafe_b64encode(token.encode()).decode().rstrip("=")
            writer.write((f"GET {parsed.path.rstrip('/')}/ws/events HTTP/1.1\r\n"
                f"Host: {parsed.netloc}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
                f"Sec-WebSocket-Protocol: codeg-events, codeg-token.{proto}\r\n\r\n").encode())
            await writer.drain()
            head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 10)
            if not head.startswith(b"HTTP/1.1 101 "):
                raise RuntimeError("WS upgrade failed: " + head.split(b"\r\n")[0].decode())
            expected = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest())
            fields = {k.strip().lower(): v.strip() for k, v in
                      (line.split(b":", 1) for line in head.split(b"\r\n")[1:] if b":" in line)}
            if fields.get(b"sec-websocket-accept") != expected:
                raise RuntimeError("invalid WS accept")
            return cls(reader, writer)
        except BaseException:
            writer.close()
            await writer.wait_closed()
            raise

    async def send(self, msg):
        self.writer.write(frame_bytes(1, json.dumps(msg).encode(), masked=True))
        await self.writer.drain()

    async def receive(self, timeout=30):
        async def next_message():
            while True:
                fin, opcode, payload = await read_frame(self.reader, expect_masked=False)
                if opcode == 8:
                    raise EOFError("WS close")
                if opcode == 9:
                    self.writer.write(frame_bytes(10, payload, masked=True))
                    await self.writer.drain()
                    continue
                if opcode in (0, 1):
                    self.fragment.extend(payload)
                    if len(self.fragment) > MAX_FRAME:
                        raise ValueError("WS message exceeds 64 MiB")
                    if fin:
                        msg = json.loads(self.fragment)
                        self.fragment.clear()
                        return msg
        return await asyncio.wait_for(next_message(), timeout)

    async def attach(self, connection, since=None):
        subscription = "e2e-" + secrets.token_hex(4)
        await self.send({"action": "attach", "subscription_id": subscription,
                         "connection_id": connection, "since_seq": since})
        while True:
            msg = await self.receive()
            if msg.get("subscription_id") == subscription:
                return msg

    async def close(self):
        self.writer.close()
        try:
            await self.writer.wait_closed()
        except (OSError, asyncio.CancelledError):
            pass


def endpoint(state, through="relay"):
    if through == "server":
        return state["server_url"], None
    if through == "proxy":
        return state["proxy_url"], state["proxy_fingerprint"]
    return state["relay_url"], state["fingerprint"]
