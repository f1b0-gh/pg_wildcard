#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Minimal ClickHouse HTTP interface backed by chDB (embedded ClickHouse), for tests.

POST / with the SQL as body; `?database=` and `?default_format=` are honoured.
Errors return HTTP 500 with the ClickHouse message. Not for production.
Arrow data is real chDB output; query-log observations are simulated test controls.
The pinned Antalya qualification in antalya.py verifies actual terminal evidence.

    python3 ch_http_shim.py --port 18123
"""
import argparse
import json
import threading
import pyarrow.ipc as ipc
import pyarrow as pa
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from chdb import session

ap = argparse.ArgumentParser()
ap.add_argument("--port", type=int, default=18123)
args = ap.parse_args()
S = session.Session()
LOCK = threading.Lock()
EVENTS = {}
TOKEN = "fdw-test-token"

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def _run(self, sql):
        q = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
        fmt = q.get("default_format", ["TabSeparated"])[0]
        db = q.get("database", [None])[0]
        try:
            if self.headers.get("Authorization") != "Bearer " + TOKEN:
                self.send_response(401); self.send_header("Content-Length", "0"); self.end_headers(); return
            if sql.rstrip().endswith("FORMAT ArrowStream"):
                sql = sql.rstrip()[:-len("FORMAT ArrowStream")].rstrip(); fmt = "ArrowStream"
            elif sql.rstrip().endswith("FORMAT JSON"):
                sql = sql.rstrip()[:-len("FORMAT JSON")].rstrip(); fmt = "JSON"
            if sql.startswith("SYSTEM FLUSH LOGS"):
                body = b""
            elif "FROM system.query_log" in sql:
                key = (q.get("param_id", [""])[0], q.get("param_nonce", [""])[0])
                row = EVENTS.get(key, {"starts": 0, "finishes": 0, "errors": 0, "cancelled": 0, "read_rows": 0, "read_bytes": 0, "result_rows": 0})
                body = json.dumps({"data": [row]}).encode()
            else:
                with LOCK:
                    if db:
                        S.query(f"CREATE DATABASE IF NOT EXISTS `{db}`")
                        S.query(f"USE `{db}`")
                    if fmt == "ArrowStream":
                        # Match the transport's negotiated IPC compression. The
                        # shared client refuses compressed bodies before decode.
                        method = q.get("output_format_arrow_compression_method", ["lz4_frame"])[0]
                        if method not in {"none", "lz4_frame", "zstd"}:
                            raise ValueError("invalid Arrow compression setting")
                        S.query(f"SET output_format_arrow_compression_method='{method}'")
                    res = S.query(sql, fmt)
                    body = res.bytes() if hasattr(res, "bytes") else res.data().encode()
                if fmt == "ArrowStream":
                    rows = sum(batch.num_rows for batch in ipc.open_stream(pa.py_buffer(body)))
                    EVENTS[(q.get("query_id", [""])[0], q.get("log_comment", [""])[0])] = {"starts": 1, "finishes": 1, "errors": 0, "cancelled": 0, "read_rows": rows, "read_bytes": 0, "result_rows": rows}
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass
        except Exception as e:  # noqa: BLE001
            if "log_comment" in q:
                EVENTS[(q.get("query_id", [""])[0], q["log_comment"][0])] = {"starts": 1, "finishes": 0, "errors": 1, "cancelled": 0, "read_rows": 0, "read_bytes": 0, "result_rows": 0}
            msg = str(e).encode()
            self.send_response(500)
            self.send_header("Content-Length", str(len(msg)))
            self.end_headers()
            self.wfile.write(msg)

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        self._run(self.rfile.read(n).decode())

    def do_GET(self):
        q = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)
        if "query" in q:
            self._run(q["query"][0])
        else:
            self.send_response(200); self.end_headers(); self.wfile.write(b"Ok.\n")

ThreadingHTTPServer(("127.0.0.1", args.port), H).serve_forever()
