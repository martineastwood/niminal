#!/usr/bin/env python3
"""Scripted OpenAI-compatible chat completions server for driving niminal offline.

Reads a scenario JSON file and answers each POST /v1/chat/completions with the
first route whose `match` appears in the newest user message. Every request body
is appended to the log file as one JSON line, which is the evidence that shows
what niminal actually sent.
"""

import argparse
import json
import os
import re
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

DEFAULT_ROUTE = {"text": "mock provider: no scenario route matched"}


def message_text(message):
    content = message.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts = []
        for part in content:
            if isinstance(part, dict) and isinstance(part.get("text"), str):
                parts.append(part["text"])
        return "\n".join(parts)
    return ""


def latest_user_text(body):
    text = ""
    for message in body.get("messages", []):
        if message.get("role") == "user":
            text = message_text(message)
    return text


class Scenario:
    def __init__(self, data):
        self.routes = data.get("routes", [])
        self.default = data.get("default", DEFAULT_ROUTE)
        self.hits = [0] * len(self.routes)
        self.lock = threading.Lock()

    def pick(self, body):
        text = latest_user_text(body)
        with self.lock:
            for index, route in enumerate(self.routes):
                pattern = route.get("match")
                if not pattern:
                    continue
                if not re.search(pattern, text):
                    continue
                responses = route.get("responses")
                if not responses:
                    return route
                hit = self.hits[index]
                self.hits[index] = min(hit + 1, len(responses) - 1)
                return responses[hit]
        return self.default


def sse(payload):
    return f"data: {json.dumps(payload)}\n\n".encode()


def chunks_for(turn, model):
    """Yield chat.completion.chunk payloads for a scripted turn."""
    base = {
        "id": "chatcmpl-mock",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": model,
    }

    def chunk(delta, finish=None, extra=None):
        payload = dict(base)
        payload["choices"] = [] if extra else [{"index": 0, "delta": delta, "finish_reason": finish}]
        if extra:
            payload.update(extra)
        return payload

    if turn.get("thinking"):
        yield chunk({"role": "assistant", "reasoning_content": turn["thinking"]})

    text = turn.get("text")
    if text:
        yield chunk({"role": "assistant", "content": text})

    calls = turn.get("tool_calls", [])
    if calls:
        deltas = []
        for index, call in enumerate(calls):
            deltas.append(
                {
                    "index": index,
                    "id": call.get("id", f"call_mock_{index}"),
                    "type": "function",
                    "function": {
                        "name": call["name"],
                        "arguments": json.dumps(call.get("input", {})),
                    },
                }
            )
        first = {"role": "assistant", "tool_calls": [deltas[0]]}
        yield chunk(first)
        for delta in deltas[1:]:
            yield chunk({"tool_calls": [delta]})
        yield chunk({}, finish="tool_calls")
        return

    yield chunk({}, finish=turn.get("finish_reason", "stop"))
    usage = turn.get("usage", {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120})
    yield chunk({}, extra={"usage": usage})


def full_response(turn, model):
    message = {"role": "assistant", "content": turn.get("text", "")}
    if turn.get("tool_calls"):
        message["content"] = turn.get("text") or None
        message["tool_calls"] = [
            {
                "id": call.get("id", f"call_mock_{index}"),
                "type": "function",
                "function": {"name": call["name"], "arguments": json.dumps(call.get("input", {}))},
            }
            for index, call in enumerate(turn["tool_calls"])
        ]
    return {
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "created": 0,
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": message,
                "finish_reason": "tool_calls" if turn.get("tool_calls") else "stop",
            }
        ],
        "usage": turn.get("usage", {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120}),
    }


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    scenario = None
    log_path = None
    log_lock = threading.Lock()

    def log_message(self, *args):
        pass

    def record(self, body):
        line = json.dumps({"time": os.times().elapsed, "path": self.path, "body": body})
        with self.log_lock:
            with open(self.log_path, "a") as handle:
                handle.write(line + "\n")
                handle.flush()

    def send_json(self, payload):
        data = json.dumps(payload).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path.endswith("/models"):
            self.send_json({"object": "list", "data": [{"id": "mock-model", "object": "model"}]})
            return
        self.send_json({"error": {"message": "mock provider: not found", "type": "not_found"}})

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(length).decode("utf-8", "replace")
        try:
            body = json.loads(raw)
        except json.JSONDecodeError:
            body = {"raw": raw}
        self.record(body)
        turn = self.scenario.pick(body)
        model = body.get("model", "mock-model")
        if not body.get("stream"):
            self.send_json(full_response(turn, model))
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()
        for payload in chunks_for(turn, model):
            self.wfile.write(sse(payload))
            self.wfile.flush()
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--scenario", required=True)
    parser.add_argument("--log", required=True)
    parser.add_argument("--state", required=True, help="write the bound port here for the driver")
    args = parser.parse_args()

    with open(args.scenario) as handle:
        Scenario.data = json.load(handle)

    Handler.scenario = Scenario(Scenario.data)
    Handler.log_path = args.log
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    port = server.server_address[1]
    with open(args.state + ".tmp", "w") as handle:
        json.dump({"pid": os.getpid(), "port": port}, handle)
    os.replace(args.state + ".tmp", args.state)
    print(f"mock provider on http://127.0.0.1:{port}/v1", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
