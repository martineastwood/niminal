"""A small example client for the niminal server: connect, send timed events, read the clock.

Everything else the server offers is reachable with `call(method, params)`."""

from __future__ import annotations

import json
from typing import Any, Callable, Optional

try:
    import websocket  # websocket-client
except ImportError as e:  # pragma: no cover
    raise ImportError("install websocket-client: pip install websocket-client") from e

PROTOCOL = 4


class NiminalError(Exception):
    def __init__(self, message: str, problems: Optional[list] = None):
        super().__init__(message)
        self.problems = problems or []


class Client:
    """WebSocket JSON-RPC client for `niminal server`."""

    def __init__(
        self,
        url: str = "ws://127.0.0.1:7401/",
        token: Optional[str] = None,
        on_notification: Optional[Callable[[str, dict], None]] = None,
    ):
        self.url = url
        self.token = token
        self.on_notification = on_notification or (lambda _m, _p: None)
        self._ws: Optional[websocket.WebSocket] = None
        self._next_id = 1

    def connect(self) -> Any:
        self._ws = websocket.create_connection(self.url, timeout=5)
        params: dict[str, Any] = {"protocol": PROTOCOL, "client": "niminal-python"}
        if self.token:
            params["token"] = self.token
        return self.call("hello", params)

    def close(self) -> None:
        if self._ws is not None:
            self._ws.close()
            self._ws = None

    def call(self, method: str, params: Optional[dict] = None) -> Any:
        assert self._ws is not None, "call connect() first"
        req_id = self._next_id
        self._next_id += 1
        self._ws.send(json.dumps({"jsonrpc": "2.0", "id": req_id, "method": method, "params": params or {}}))
        while True:
            raw = self._ws.recv()
            message = json.loads(raw)
            if message.get("id") == req_id:
                if "error" in message:
                    err = message["error"]
                    raise NiminalError(err.get("message", "error"), err.get("data", {}).get("problems"))
                return message.get("result")
            if message.get("method"):
                self.on_notification(message["method"], message.get("params") or {})

    def status(self) -> Any:
        return self.call("status")

    def send_events(self, events: list[dict]) -> Any:
        """Send timed events: `{"op": "note" | "control" | "track", "at": ..., ...}`.

        `at` is an absolute sample count or "now".
        """
        return self.call("events.send", {"events": events})

    def cancel_events(self, tag: Optional[str] = None, from_: Any = None, release: Optional[bool] = None) -> Any:
        """Take back this client's waiting events."""
        params: dict[str, Any] = {}
        if tag is not None:
            params["tag"] = tag
        if from_ is not None:
            params["from"] = from_
        if release is not None:
            params["release"] = release
        return self.call("events.cancel", params)

    def clock(self) -> Any:
        """Where the engine is in time: sample, sample_rate, prepared_until, wall_micros..."""
        return self.call("clock")

    def panic(self) -> Any:
        return self.call("panic")
