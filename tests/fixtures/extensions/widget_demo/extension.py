#!/usr/bin/env python3
import json
import sys

jobs = [
    {"text": "First task", "state": "done"},
    {"text": "Second task", "state": "active"},
]


def send(value):
    print(json.dumps(value), flush=True)


def widget():
    return {"key": "workers", "title": "Widget demo",
            "content": [{"type": "list", "items": jobs}],
            "actions": [
                {"id": "steer", "label": "Steer active task"},
                {"id": "stop", "label": "Stop active task"},
            ]}


send({"type": "register", "commands": [
    {"name": "subagents_demo", "description": "Show a widget action demo"}
]})

for line in sys.stdin:
    message = json.loads(line)
    kind = message.get("type")
    if kind == "shutdown":
        break
    if kind == "initialize":
        continue
    if kind == "command":
        send({"type": "response", "id": message["id"], "widget": widget(),
              "message": "Showing simulated subagent activity."})
    elif kind == "ui_action" and message.get("widget") == "workers":
        if message.get("action") == "stop":
            jobs[1]["text"] = "Second task · stopped"
            jobs[1]["state"] = "done"
        send({"type": "update", "widget": widget()})
