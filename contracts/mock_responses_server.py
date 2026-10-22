#!/usr/bin/env python3
"""Serve a contract's scripted model responses over Responses-compatible SSE."""

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import Lock


def completed(response_id):
    return {
        "type": "response.completed",
        "response": {
            "id": response_id,
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0,
                "total_tokens": 0,
            },
        },
    }


def as_events(index, scripted):
    events = []
    for call in scripted.get("tool_calls", []):
        events.append(
            {
                "type": "response.output_item.done",
                "item": {
                    "type": "function_call",
                    "call_id": call["call_id"],
                    "name": call["name"],
                    "arguments": json.dumps(call["arguments"]),
                },
            }
        )
    if scripted.get("text") is not None:
        events.append(
            {
                "type": "response.output_item.done",
                "item": {
                    "type": "message",
                    "role": "assistant",
                    "id": "message-{}".format(index),
                    "content": [
                        {"type": "output_text", "text": scripted["text"]}
                    ],
                },
            }
        )
    events.append(completed("response-{}".format(index)))
    return events


def encode_sse(events):
    return "".join(
        "event: {0}\ndata: {1}\n\n".format(event["type"], json.dumps(event))
        for event in events
    ).encode("utf-8")


class State:
    def __init__(self, contract, request_log):
        self.responses = contract["responses"]
        self.request_log = request_log
        self.index = 0
        self.lock = Lock()

    def next(self, request):
        with self.lock:
            index = self.index
            self.index += 1
            with self.request_log.open("a", encoding="utf-8") as output:
                output.write(json.dumps({"request_index": index, "body": request}) + "\n")
        if index >= len(self.responses):
            return 500, json.dumps({"error": "unexpected extra request"}).encode()
        return 200, encode_sse(as_events(index, self.responses[index]))


def handler_for(state):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_POST(self):
            length = int(self.headers.get("content-length", "0"))
            request = json.loads(self.rfile.read(length) or b"{}")
            status, body = state.next(request)
            self.send_response(status)
            self.send_header(
                "content-type",
                "text/event-stream" if status == 200 else "application/json",
            )
            self.send_header("content-length", str(len(body)))
            self.send_header("connection", "close")
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format, *args):
            return

    return Handler


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--contract", type=Path, required=True)
    parser.add_argument("--port-file", type=Path, required=True)
    parser.add_argument("--request-log", type=Path, required=True)
    args = parser.parse_args()
    contract = json.loads(args.contract.read_text(encoding="utf-8"))
    server = ThreadingHTTPServer(
        ("127.0.0.1", 0), handler_for(State(contract, args.request_log))
    )
    args.port_file.write_text(str(server.server_port), encoding="utf-8")
    server.serve_forever()


if __name__ == "__main__":
    main()
