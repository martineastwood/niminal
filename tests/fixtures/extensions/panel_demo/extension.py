#!/usr/bin/env python3
import json
import sys

rows = 40


def send(value):
    print(json.dumps(value), flush=True)


def body(count):
    return "\n".join(f"row {i:02d}" for i in range(1, count + 1))


def widget(count):
    return {"key": "body", "title": "Panel demo",
            "content": [{"type": "markdown", "text": body(count), "height": 6}],
            "actions": [{"id": "more", "label": "More"}, {"id": "close", "label": "Close"}]}


def invalid():
    return {"key": "invalid", "title": "Invalid panel",
            "content": [{"type": "markdown", "text": "","height": "tall"},
                        {"type": "chart", "series": [1, 2]}],
            "actions": []}


send({"type": "register", "commands": [
    {"name": "panel", "description": "Show a markdown panel"},
    {"name": "panel_invalid", "description": "Show a panel the host must reject"},
]})

for line in sys.stdin:
    message = json.loads(line)
    kind = message.get("type")
    if kind == "shutdown":
        break
    if kind == "command" and message.get("name") == "panel":
        send({"type": "response", "id": message["id"], "widget": widget(rows)})
    elif kind == "command" and message.get("name") == "panel_invalid":
        send({"type": "response", "id": message["id"], "widget": invalid()})
    elif kind == "ui_action" and message.get("widget") == "body":
        if message.get("action") == "more":
            rows += 20
            send({"type": "update", "widget": widget(rows)})
        elif message.get("action") == "close":
            send({"type": "update",
                  "widget": {"key": "body", "title": "", "content": [], "actions": []}})
