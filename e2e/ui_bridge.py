#!/usr/bin/env python3
"""Disposable loopback bridge for the E1b XCUITest driver; never logs commands/secrets."""
import argparse
import base64
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import queue
import threading
import time
import uuid

from harness import load_state

commands = queue.Queue()
results = {}
condition = threading.Condition()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, value, content_type='application/json'):
        data = json.dumps(value).encode() if content_type == 'application/json' else value
        self.send_response(200)
        self.send_header('Content-Type', content_type)
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == '/next':
            try:
                self.reply(commands.get(timeout=20))
            except queue.Empty:
                self.reply({'action': 'noop'})
        elif self.path == '/secret':
            self.reply(load_state('e1b')['token'].encode(), 'text/plain')
        else:
            self.reply({'error': 'unknown endpoint'})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if self.path == '/result':
            # Secure fields normally mask values; redact the disposable token defensively.
            token = load_state('e1b').get('token')
            if token:
                body['tree'] = body.get('tree', '').replace(token, '[REDACTED]')
            with condition:
                results[body['id']] = body
                condition.notify_all()
            self.reply({'ok': True})
        elif self.path == '/command':
            body['id'] = str(uuid.uuid4())
            commands.put(body)
            deadline = time.monotonic() + 60
            with condition:
                while body['id'] not in results and time.monotonic() < deadline:
                    condition.wait(1)
                self.reply(results.pop(body['id'], {'error': 'driver timeout', 'id': body['id']}))
        else:
            self.reply({'error': 'unknown endpoint'})


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--config', required=True)
    parser.add_argument('--port', type=int, default=0)
    args = parser.parse_args()
    server = ThreadingHTTPServer(('127.0.0.1', args.port), Handler)
    Path(args.config).write_text(json.dumps({'url': f'http://127.0.0.1:{server.server_port}'}))
    print(f'E1b test bridge listening on loopback port {server.server_port}', flush=True)
    server.serve_forever()
