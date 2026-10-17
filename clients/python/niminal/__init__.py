"""Typed client for the niminal daemon performance API, plus offline .nms builders."""

from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field
from typing import Any, Callable, Optional
from urllib.parse import urlparse

try:
    import websocket  # websocket-client
except ImportError as e:  # pragma: no cover
    raise ImportError("install websocket-client: pip install websocket-client") from e

PROTOCOL = 1


class NiminalError(Exception):
    def __init__(self, message: str, problems: Optional[list] = None):
        super().__init__(message)
        self.problems = problems or []


@dataclass
class ClipDef:
    name: str
    instrument: Optional[str] = None
    length: Optional[str] = None
    loop: bool = True
    lanes: dict[str, str] = field(default_factory=dict)
    events: list[dict[str, Any]] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {"name": self.name}
        if self.instrument is not None:
            d["instrument"] = self.instrument
        if self.length is not None:
            d["length"] = self.length
        if not self.loop:
            d["loop"] = False
        if self.lanes:
            d["lanes"] = self.lanes
        if self.events:
            d["events"] = self.events
        return d


@dataclass
class SceneDef:
    name: str
    entries: dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return {"name": self.name, "entries": self.entries}


@dataclass
class Project:
    clips: list[ClipDef] = field(default_factory=list)
    scenes: list[SceneDef] = field(default_factory=list)
    arrangement: Optional[dict[str, Any]] = None

    def to_json(self) -> str:
        body: dict[str, Any] = {
            "version": 2,
            "clips": [c.to_dict() for c in self.clips],
            "scenes": [s.to_dict() for s in self.scenes],
        }
        if self.arrangement is not None:
            body["arrangement"] = self.arrangement
        return json.dumps(body, indent=2)


class Client:
    """WebSocket JSON-RPC client for `niminal daemon`."""

    def __init__(
        self,
        url: str = "ws://127.0.0.1:7400/",
        token: Optional[str] = None,
        on_notification: Optional[Callable[[str, dict], None]] = None,
    ):
        self.url = url
        self.token = token
        self.on_notification = on_notification or (lambda _m, _p: None)
        self._ws: Optional[websocket.WebSocket] = None
        self._next_id = 1

    def connect(self) -> None:
        self._ws = websocket.create_connection(self.url, timeout=5)
        params: dict[str, Any] = {"protocol": PROTOCOL, "client": "niminal-python"}
        if self.token:
            params["token"] = self.token
        self.call("hello", params)

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

    def eval(self, source: str, quantize: Optional[str] = None, dir: Optional[str] = None) -> Any:
        params: dict[str, Any] = {"source": source}
        if quantize:
            params["quantize"] = quantize
        if dir:
            params["dir"] = dir
        return self.call("eval", params)

    def define_clip(self, clip: ClipDef | dict, quantize: Optional[str] = None) -> Any:
        params = clip.to_dict() if isinstance(clip, ClipDef) else dict(clip)
        if quantize:
            params["quantize"] = quantize
        return self.call("clip.define", params)

    def define_scene(self, scene: SceneDef | dict, quantize: Optional[str] = None) -> Any:
        params = scene.to_dict() if isinstance(scene, SceneDef) else dict(scene)
        if quantize:
            params["quantize"] = quantize
        return self.call("scene.define", params)

    def launch(self, target: str, quantize: Optional[str] = None) -> Any:
        params: dict[str, Any] = {"target": target}
        if quantize:
            params["quantize"] = quantize
        return self.call("launch", params)

    def stop(self, target: str, quantize: Optional[str] = None) -> Any:
        params: dict[str, Any] = {"target": target}
        if quantize:
            params["quantize"] = quantize
        return self.call("stop", params)

    def edit(self, track: str, change: str, quantize: Optional[str] = None) -> Any:
        params: dict[str, Any] = {"track": track, "change": change}
        if quantize:
            params["quantize"] = quantize
        return self.call("edit", params)

    def hush(self) -> Any:
        return self.call("hush")

    def panic(self) -> Any:
        return self.call("panic")

    def status(self) -> Any:
        return self.call("status")

    def send_events(self, events: list[dict]) -> Any:
        """Send timed events: `{"op": "note" | "control" | "track", "at": ..., ...}`.

        `at` is a sample, "now" or a position such as "32beat".
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
        """Where the engine is in time: sample, bar, beat, bpm, prepared_until, wall_micros..."""
        return self.call("clock")

    def set_control(self, name: str, value: float, unit: Optional[str] = None, at: Any = "now", glide: Optional[str] = None) -> Any:
        op: dict[str, Any] = {"op": "control", "name": name, "value": value, "at": at}
        if unit is not None:
            op["unit"] = unit
        if glide is not None:
            op["glide"] = glide
        return self.send_events([op])


# Back-compat alias used in the language spec examples.
Session = Client

__all__ = [
    "PROTOCOL",
    "NiminalError",
    "ClipDef",
    "SceneDef",
    "Project",
    "Client",
    "Session",
]
