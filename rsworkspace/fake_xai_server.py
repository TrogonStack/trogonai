#!/usr/bin/env python3
"""Minimal fake xAI Responses API server for smoke-testing trogon-xai-runner.

Modes (set via ?mode=... query param):
  normal   — returns a valid SSE response stream (default)
  hang     — accepts the request but never sends any bytes
  error    — returns HTTP 500

Usage:
  python3 fake_xai_server.py <port> [normal|hang|error]
"""
import sys
import json
import time
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

DEFAULT_MODE = "normal"

def make_sse_response(resp_id="resp-fake-001"):
    lines = [
        f'data: {json.dumps({"id": resp_id, "type": "message.delta", "delta": {"text": "Hello from fake xAI"}})}\n\n',
        f'data: {json.dumps({"type": "response.completed", "id": resp_id, "usage": {"prompt_tokens": 10, "completion_tokens": 5}, "status": "completed", "response": {"id": resp_id, "usage": {"prompt_tokens": 10, "completion_tokens": 5}}})}\n\n',
        "data: [DONE]\n\n",
    ]
    return "".join(lines).encode()


class Handler(BaseHTTPRequestHandler):
    mode = DEFAULT_MODE
    request_log = []
    lock = threading.Lock()

    def log_message(self, fmt, *args):
        pass  # silence default access log

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length) if length else b""

        with Handler.lock:
            Handler.request_log.append({
                "path": self.path,
                "body": body.decode(errors="replace"),
            })

        mode = self.mode
        # also allow per-request override via query param
        if "?mode=" in self.path:
            mode = self.path.split("?mode=")[-1]

        if mode == "hang":
            # accept the connection but never send HTTP headers (simulates TCP stall)
            time.sleep(120)
            self.send_response(200)
            self.end_headers()
            return

        if mode == "hang_stream":
            # send HTTP 200 + SSE headers, then stall mid-stream (no events)
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()
            try:
                self.wfile.flush()
                time.sleep(120)  # hold open without sending any SSE events
            except Exception:
                pass
            return

        if mode == "error":
            self.send_response(500)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(b"Internal Server Error")
            return

        # normal: send a short SSE stream
        body_bytes = make_sse_response()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Content-Length", str(len(body_bytes)))
        self.end_headers()
        self.wfile.write(body_bytes)

    def do_GET(self):
        """GET /log — return all captured request bodies as JSON."""
        with Handler.lock:
            data = json.dumps(Handler.request_log).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 9876
    mode = sys.argv[2] if len(sys.argv) > 2 else DEFAULT_MODE
    Handler.mode = mode

    server = HTTPServer(("127.0.0.1", port), Handler)
    print(f"fake-xai listening on 127.0.0.1:{port} mode={mode}", flush=True)
    server.serve_forever()
