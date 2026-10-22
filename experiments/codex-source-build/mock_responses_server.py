#!/usr/bin/env python3
"""Deterministic local Responses API for exercising the real Codex tool loop."""

from __future__ import annotations

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import Lock


PATCH = """*** Begin Patch
*** Update File: calculator.py
@@
 def add(left: int, right: int) -> int:
     \"\"\"Return the sum of two integers.\"\"\"
-    return left - right
+    return left + right
*** End Patch"""


def sse(events: list[dict]) -> bytes:
    chunks = []
    for event in events:
        kind = event["type"]
        chunks.append(f"event: {kind}\ndata: {json.dumps(event)}\n\n")
    return "".join(chunks).encode("utf-8")


def completed(response_id: str) -> dict:
    return {
        "type": "response.completed",
        "response": {
            "id": response_id,
            "usage": {
                "input_tokens": 0,
                "input_tokens_details": None,
                "output_tokens": 0,
                "output_tokens_details": None,
                "total_tokens": 0,
            },
        },
    }


RESPONSES = [
    [
        {
            "type": "response.output_item.done",
            "item": {
                "type": "function_call",
                "call_id": "call-test-before",
                "name": "exec_command",
                "arguments": json.dumps(
                    {
                        "cmd": "python3 -m unittest discover -s . -p 'test_*.py'",
                        "yield_time_ms": 10000,
                    }
                ),
            },
        },
        completed("response-1"),
    ],
    [
        {
            "type": "response.output_item.done",
            "item": {
                "type": "function_call",
                "call_id": "call-fix",
                "name": "exec_command",
                "arguments": json.dumps(
                    {"cmd": "apply_patch <<'PATCH'\n" + PATCH + "\nPATCH"}
                ),
            },
        },
        completed("response-2"),
    ],
    [
        {
            "type": "response.output_item.done",
            "item": {
                "type": "function_call",
                "call_id": "call-test-after",
                "name": "exec_command",
                "arguments": json.dumps(
                    {
                        "cmd": "python3 -m unittest discover -s . -p 'test_*.py'",
                        "yield_time_ms": 10000,
                    }
                ),
            },
        },
        completed("response-3"),
    ],
    [
        {
            "type": "response.output_item.done",
            "item": {
                "type": "message",
                "role": "assistant",
                "id": "message-1",
                "content": [
                    {
                        "type": "output_text",
                        "text": "Fixed calculator.add and verified both tests pass.",
                    }
                ],
            },
        },
        completed("response-4"),
    ],
]


class State:
    def __init__(self, log_path: Path) -> None:
        self.log_path = log_path
        self.lock = Lock()
        self.index = 0

    def next_response(self, request: dict) -> tuple[int, bytes]:
        with self.lock:
            index = self.index
            self.index += 1
            with self.log_path.open("a", encoding="utf-8") as log:
                log.write(json.dumps({"request_index": index, "body": request}) + "\n")
        if index >= len(RESPONSES):
            return 500, json.dumps({"error": "unexpected extra request"}).encode("utf-8")
        return 200, sse(RESPONSES[index])


def make_handler(state: State):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_POST(self) -> None:
            length = int(self.headers.get("content-length", "0"))
            request = json.loads(self.rfile.read(length) or b"{}")
            status, body = state.next_response(request)
            self.send_response(status)
            self.send_header(
                "content-type",
                "text/event-stream" if status == 200 else "application/json",
            )
            self.send_header("content-length", str(len(body)))
            self.send_header("connection", "close")
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format: str, *args: object) -> None:
            return

    return Handler


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--port-file", type=Path, required=True)
    parser.add_argument("--request-log", type=Path, required=True)
    args = parser.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(State(args.request_log)))
    args.port_file.write_text(str(server.server_port), encoding="utf-8")
    server.serve_forever()


if __name__ == "__main__":
    main()
