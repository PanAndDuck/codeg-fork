import asyncio
import importlib.util
import json
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from harness import frame_bytes, read_frame


class WireTests(unittest.IsolatedAsyncioTestCase):
    async def test_masked_large_frame_and_fragmentation(self):
        for n in (0, 125, 126, 65535, 65536, 1024 * 1024 + 1):
            reader = asyncio.StreamReader()
            payload = b"x" * n
            reader.feed_data(frame_bytes(1, payload, masked=True, fin=False))
            reader.feed_eof()
            self.assertEqual(await read_frame(reader), (False, 1, payload))

    async def test_rejects_fragmented_or_oversized_control_frames(self):
        for payload, fin in ((b"x", False), (b"x" * 126, True)):
            reader = asyncio.StreamReader()
            reader.feed_data(frame_bytes(9, payload, fin=fin))
            reader.feed_eof()
            with self.assertRaises(ValueError):
                await read_frame(reader)


class ProxyTests(unittest.IsolatedAsyncioTestCase):
    async def test_fragmented_attach_rewrite_preserves_interleaved_ping(self):
        spec = importlib.util.spec_from_file_location("faultproxy_fragments", ROOT / "faultproxy/faultproxy.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        proxy = module.Proxy(mode="terminate", upstream_port=18443, fingerprint="")
        await proxy.control({"command": "omit_since_seq", "count": 1})

        class Writer:
            def __init__(self):
                self.data = bytearray()

            def write(self, data):
                self.data.extend(data)

            async def drain(self):
                pass

        writer = Writer()
        connection = module.Connection(writer)
        payload = json.dumps({"action": "attach", "subscription_id": "s", "connection_id": "c", "since_seq": 7}).encode()
        reader = asyncio.StreamReader()
        reader.feed_data(frame_bytes(1, payload[:20], masked=True, fin=False))
        reader.feed_data(frame_bytes(9, b"ping", masked=True))
        reader.feed_data(frame_bytes(0, payload[20:], masked=True))
        reader.feed_eof()
        with self.assertRaises(asyncio.IncompleteReadError):
            await proxy.ws_pipe(reader, writer, connection, False)
        output = asyncio.StreamReader()
        output.feed_data(writer.data)
        output.feed_eof()
        self.assertEqual(await read_frame(output, expect_masked=True), (True, 9, b"ping"))
        fin, opcode, rewritten = await read_frame(output, expect_masked=True)
        self.assertTrue(fin)
        self.assertEqual(opcode, 1)
        self.assertEqual(json.loads(rewritten), {"action": "attach", "subscription_id": "s", "connection_id": "c"})

    async def test_fault_controls_and_passthrough_scope(self):
        path = ROOT / "faultproxy/faultproxy.py"
        self.assertTrue(path.exists(), "fault proxy must exist")
        spec = importlib.util.spec_from_file_location("faultproxy", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        proxy = module.Proxy(mode="passthrough", upstream_port=18443, fingerprint="")
        with self.assertRaises(ValueError):
            await proxy.control({"command": "omit_since_seq"})
        await proxy.control({"command": "throttle", "bps": 1024})
        self.assertEqual(proxy.status()["throttle_bps"], 1024)
        await proxy.control({"command": "reset"})
        self.assertEqual(proxy.status()["throttle_bps"], 0)
        with self.assertRaises(ValueError):
            await proxy.control({"command": "block_ws", "seconds": -1})

    async def test_terminated_http_preserves_body_and_drops_response_once(self):
        path = ROOT / "faultproxy/faultproxy.py"
        self.assertTrue(path.exists(), "fault proxy must exist")
        spec = importlib.util.spec_from_file_location("faultproxy_http", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        received = []

        async def upstream(reader, writer):
            head = await reader.readuntil(b"\r\n\r\n")
            size = int(head.lower().split(b"content-length: ")[1].split(b"\r\n")[0])
            received.append(await reader.readexactly(size))
            writer.write(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nnull")
            await writer.drain()
            writer.close()
            await writer.wait_closed()

        server = await asyncio.start_server(upstream, "127.0.0.1", 0)
        self.addAsyncCleanup(server.wait_closed)
        self.addCleanup(server.close)
        port = server.sockets[0].getsockname()[1]
        proxy = module.Proxy(mode="terminate", upstream_port=port, fingerprint="")
        # Only the upstream dial uses plaintext in this in-process fixture.
        async def dial():
            return await asyncio.open_connection("127.0.0.1", port)
        proxy.dial = dial
        listener = await asyncio.start_server(proxy.accept, "127.0.0.1", 0)
        self.addAsyncCleanup(listener.wait_closed)
        self.addCleanup(listener.close)
        self.addAsyncCleanup(proxy.close)
        await proxy.control({"command": "drop_http_response", "path": "/api/acp_prompt"})
        for i in range(2):
            reader, writer = await asyncio.open_connection("127.0.0.1", listener.sockets[0].getsockname()[1])
            body = json.dumps({"prompt": i}).encode()
            writer.write(b"POST /api/acp_prompt HTTP/1.1\r\nHost: test\r\nContent-Length: " +
                         str(len(body)).encode() + b"\r\n\r\n" + body)
            await writer.drain()
            response = await asyncio.wait_for(reader.read(), 3)
            self.assertEqual(bool(response), i == 1)
            writer.close()
            await writer.wait_closed()
        self.assertEqual(len(received), 2)
        self.assertEqual(proxy.status()["counters"]["dropped_http_responses"], 1)


if __name__ == "__main__":
    unittest.main()
