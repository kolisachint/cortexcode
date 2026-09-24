#!/usr/bin/env python3
"""Scripted, deterministic LLM server shared by hoocode and cortex parity runs.

Speaks the OpenAI Chat Completions streaming protocol (``POST /v1/chat/completions``),
which both apps reach through a custom ``models.json`` provider (api
``openai-completions``). Each request is answered with the next scripted turn, so
both apps see byte-identical model output.

Script format (JSON list, one entry per model request)::

    [
      {"text": "Hello!"},
      {"thinking": "...", "text": "Reading", "tool_calls": [{"name": "read", "arguments": {"path": "a.txt"}}]},
      {"text": "Done."}
    ]

Every request body is appended to ``--log`` as one JSON line, so level-1 checks can
diff what each app actually sent to the model.

Stdlib only. Usage::

    mockllm.py --script turns.json --port 0 --port-file port.txt --log requests.jsonl
"""

from __future__ import annotations

import argparse
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CHUNK_DELAY_S = 0.01


class State:
    def __init__(self, turns: list[dict], log_path: str | None, chunk_size: int) -> None:
        self.turns = turns
        self.index = 0
        self.lock = threading.Lock()
        self.log_path = log_path
        self.chunk_size = chunk_size

    def next_turn(self) -> dict:
        with self.lock:
            if self.index < len(self.turns):
                turn = self.turns[self.index]
            else:
                turn = {"text": "[mockllm: script exhausted]"}
            self.index += 1
            return turn

    def log(self, body: dict) -> None:
        if not self.log_path:
            return
        with self.lock, open(self.log_path, "a", encoding="utf-8") as f:
            f.write(json.dumps(body, sort_keys=True) + "\n")


def chunks(text: str, size: int) -> list[str]:
    return [text[i : i + size] for i in range(0, len(text), size)] or [""]


def make_handler(state: State):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_args) -> None:  # silence default stderr logging
            pass

        def _json(self, status: int, payload: dict) -> None:
            data = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self) -> None:
            if self.path.rstrip("/").endswith("/models"):
                self._json(200, {"object": "list", "data": [{"id": "mock-model", "object": "model"}]})
            elif self.path == "/health":
                self._json(200, {"ok": True, "served": state.index})
            else:
                self._json(404, {"error": "not found"})

        def do_POST(self) -> None:
            length = int(self.headers.get("Content-Length", "0"))
            raw = self.rfile.read(length) if length else b"{}"
            try:
                body = json.loads(raw or b"{}")
            except json.JSONDecodeError:
                body = {"_unparseable": raw.decode(errors="replace")}
            state.log({"path": self.path, "body": body})

            if not self.path.rstrip("/").endswith("/chat/completions"):
                self._json(404, {"error": f"unsupported path {self.path}"})
                return

            turn = state.next_turn()
            if turn.get("error"):
                self._json(int(turn.get("status", 500)), {"error": {"message": turn["error"]}})
                return
            if not body.get("stream", False):
                self._json(400, {"error": {"message": "mockllm only supports stream=true"}})
                return
            self._stream(turn, body)

        def _send(self, payload: dict | str) -> None:
            line = payload if isinstance(payload, str) else json.dumps(payload)
            data = f"data: {line}\n\n".encode()
            self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n")
            self.wfile.flush()
            time.sleep(CHUNK_DELAY_S)

        def _stream(self, turn: dict, body: dict) -> None:
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.send_header("Transfer-Encoding", "chunked")
            self.end_headers()

            model = body.get("model", "mock-model")
            base = {"id": "chatcmpl-mock", "object": "chat.completion.chunk", "created": 0, "model": model}

            def delta(d: dict, finish: str | None = None) -> dict:
                return {**base, "choices": [{"index": 0, "delta": d, "finish_reason": finish}]}

            self._send(delta({"role": "assistant", "content": ""}))
            if turn.get("thinking"):
                for part in chunks(turn["thinking"], state.chunk_size):
                    self._send(delta({"reasoning_content": part}))
            if turn.get("text"):
                for part in chunks(turn["text"], state.chunk_size):
                    self._send(delta({"content": part}))
            calls = turn.get("tool_calls") or []
            for i, call in enumerate(calls):
                call_id = call.get("id", f"call_{i}")
                args = json.dumps(call.get("arguments", {}))
                self._send(
                    delta(
                        {
                            "tool_calls": [
                                {
                                    "index": i,
                                    "id": call_id,
                                    "type": "function",
                                    "function": {"name": call["name"], "arguments": ""},
                                }
                            ]
                        }
                    )
                )
                for part in chunks(args, state.chunk_size):
                    self._send(delta({"tool_calls": [{"index": i, "function": {"arguments": part}}]}))
            usage = turn.get("usage", {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120})
            self._send({**delta({}, "tool_calls" if calls else "stop"), "usage": usage})
            self._send("[DONE]")
            self.wfile.write(b"0\r\n\r\n")
            self.wfile.flush()

    return Handler


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--script", required=True, help="JSON file with the list of scripted turns")
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--port-file", help="write the bound port here once listening")
    ap.add_argument("--log", help="append each request body as JSONL")
    ap.add_argument("--chunk-size", type=int, default=8, help="characters per streamed delta")
    args = ap.parse_args()

    with open(args.script, encoding="utf-8") as f:
        turns = json.load(f)
    state = State(turns, args.log, args.chunk_size)
    server = ThreadingHTTPServer(("127.0.0.1", args.port), make_handler(state))
    if args.port_file:
        with open(args.port_file, "w") as f:
            f.write(str(server.server_address[1]))
    server.serve_forever()


if __name__ == "__main__":
    main()
