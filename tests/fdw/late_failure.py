#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Container-local late-error HTTP fixture; private configuration via stdin."""
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

config = json.loads(sys.stdin.readline())
arrow = (Path(__file__).parent / 'one-u64.arrow').read_bytes()
observations = []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        sql = self.rfile.read(int(self.headers.get('Content-Length', '0')))
        if self.headers.get('Authorization') != 'Bearer ' + config['token']:
            self.send_response(401)
            self.end_headers()
            return
        if b'system.query_log' in sql:
            observations.append(1)
            body = b'{"data":[{"starts":1,"finishes":0,"errors":1,"cancelled":0,"read_rows":1,"read_bytes":8,"result_rows":1}]}'
        elif b'SYSTEM FLUSH LOGS' in sql:
            body = b''
        else:
            body = arrow
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)


server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
print(server.server_port, flush=True)
try:
    for command in sys.stdin:
        if command.strip() == 'observations':
            print(len(observations), flush=True)
        elif command.strip() == 'stop':
            break
finally:
    server.shutdown()
    server.server_close()
    thread.join(timeout=2)
