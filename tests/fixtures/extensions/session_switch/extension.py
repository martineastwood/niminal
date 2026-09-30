#!/usr/bin/env python3
"""Record what a session change tells a loaded extension.

Writing a line at startup lets a test tell a session change that keeps this
process from one that restarted it, and the event log records the lifecycle
events with the session id and the provider, model, and thinking level the
message carried.
"""
import json
import os
import sys


def append(name, line):
    with open(os.path.join(os.environ.get("HOME", "."), name), "a") as out:
        out.write(line + "\n")


append("session_switch_starts.log", "start")

print(json.dumps({"type": "register", "commands": [],
                  "events": ["session_shutdown", "session_end", "session_start",
                             "session_settings_changed"]}),
      flush=True)

for line in sys.stdin:
    message = json.loads(line)
    kind = message.get("type")
    if kind == "shutdown":
        break
    if kind == "initialize":
        continue
    if kind == "event":
        payload = message.get("payload") or {}
        parts = [message.get("event", ""), payload.get("session_id", "")]
        if payload.get("reason"):
            parts.append(payload["reason"])
        if payload.get("provider"):
            parts.append(payload["provider"])
        if payload.get("model"):
            parts.append(payload["model"])
        if payload.get("thinking"):
            parts.append(payload["thinking"])
        append("session_switch_events.log", ":".join(parts))
    print(json.dumps({"type": "response", "id": message.get("id", "")}), flush=True)
