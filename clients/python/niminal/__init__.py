"""Typed client for the niminal daemon performance API, plus offline .nms builders."""

from __future__ import annotations

import json
import base64
from pathlib import Path
from dataclasses import asdict, dataclass, field
from typing import Any, Callable, Optional
from urllib.parse import urlparse

try:
    import websocket  # websocket-client
except ImportError as e:  # pragma: no cover
    raise ImportError("install websocket-client: pip install websocket-client") from e

PROTOCOL = 4


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

    def routing(self, edits: list[dict], at: int | str = "now", tag: Optional[str] = None) -> Any:
        """Edit this connection's processor instances, buses and connections."""
        event = {"op": "routing", "at": at, "edits": edits}
        if tag is not None:
            event["tag"] = tag
        return self.send_events([event])

    def transaction(self, source: str = "", edits: Optional[list[dict]] = None, at: int | str = "now", dir: Optional[str] = None, tag: Optional[str] = None) -> Any:
        """Compile definitions and edit routing as one scheduled graph change."""
        event = {"op": "transaction", "source": source, "edits": edits or [], "at": at}
        if dir is not None:
            event["dir"] = dir
        if tag is not None:
            event["tag"] = tag
        return self.send_events([event])

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

    def set_control(self, name: str, value: float, unit: Optional[str] = None, at: Any = "now", glide: Optional[str] = None) -> Any:
        op: dict[str, Any] = {"op": "control", "name": name, "value": value, "at": at}
        if unit is not None:
            op["unit"] = unit
        if glide is not None:
            op["glide"] = glide
        return self.send_events([op])


    def panic(self) -> Any:
        return self.call("panic")

    def panic_engine(self) -> Any:
        return self.call("engine.panic")

    def create_scope(self, id: str, endpoint: dict, frames: int = 1024, rate: float = 20) -> Any:
        return self.call("scopes.create", {"id": id, "endpoint": endpoint, "frames": frames, "rate": rate})

    def remove_scope(self, id: str) -> Any:
        return self.call("scopes.remove", {"id": id})

    def buffer_budget(self, bytes: Optional[int] = None) -> Any:
        return self.call("buffers.budget", {} if bytes is None else {"bytes": bytes})

    def list_buffers(self) -> Any:
        return self.call("buffers.list")

    def alloc_buffer(self, name: str, frames: int, channels: int = 1) -> Any:
        return self.call("buffers.alloc", {"name": name, "frames": frames, "channels": channels})

    def free_buffer(self, name: str) -> Any:
        return self.call("buffers.free", {"name": name})

    def load_buffer(self, name: str, path: str, channel: int = 0) -> Any:
        return self.call("buffers.load", {"name": name, "path": path, "channel": channel})

    def read_buffer(self, name: str, count: int, offset: int = 0) -> Any:
        return self.call("buffers.read", {"name": name, "count": count, "offset": offset})

    def save_buffer(self, name: str, path: str) -> Any:
        return self.call("buffers.save", {"name": name, "path": path})

    def render(self, **spec: Any) -> Any:
        return self.call("render.start", spec)

    def job_status(self, id: int) -> Any:
        return self.call("jobs.status", {"id": id})

    def cancel_job(self, id: int) -> Any:
        return self.call("jobs.cancel", {"id": id})

    def capture(self, output: dict, at: int | str = "now") -> Any:
        return self.call("capture.start", {"output": output, "at": at})

    def stop_capture(self, id: int, at: int | str = "now") -> Any:
        return self.call("capture.stop", {"id": id, "at": at})

    def audio_devices(self) -> Any:
        return self.call("audio.devices")

    def audio_status(self) -> Any:
        return self.call("audio.status")

    def configure_audio(self, settings: dict, restart: bool = False) -> Any:
        return self.call("audio.configure", {"settings": settings, "restart": restart})

    def remove_file(self, id: str) -> Any:
        return self.call("files.remove", {"id": id})

    def upload_file(self, path: str | Path) -> str:
        id = None
        offset = 0
        with open(path, "rb") as file:
            while True:
                data = file.read(262144)
                finish = offset + len(data) == Path(path).stat().st_size
                params = {"offset": offset, "data": base64.b64encode(data).decode("ascii"), "finish": finish}
                if id is not None:
                    params["id"] = id
                id = self.call("files.upload", params)["id"]
                offset += len(data)
                if finish:
                    return id

    def download_file(self, id: str, path: str | Path) -> None:
        offset = 0
        with open(path, "wb") as file:
            while True:
                result = self.call("files.read", {"id": id, "offset": offset, "count": 262144})
                data = base64.b64decode(result["data"])
                file.write(data)
                offset += len(data)
                if result["eof"]:
                    return


class LiveClient(Client):
    """Control a separate `niminal live` sequencer process."""

    def __init__(self, url: str = "ws://127.0.0.1:7400/", **kwargs):
        super().__init__(url=url, **kwargs)

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


# Back-compat alias used in the language spec examples.
Session = Client

__all__ = [
    "PROTOCOL",
    "NiminalError",
    "ClipDef",
    "SceneDef",
    "Project",
    "Client",
    "LiveClient",
    "Session",
]
