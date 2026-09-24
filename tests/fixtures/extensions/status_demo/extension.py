#!/usr/bin/env python3
import json
import os
import sys


def send(value):
    print(json.dumps(value), flush=True)


send({"type": "register", "commands": [
    {"name": "footer_demo", "description": "Exercise footer status updates"}
]})

for line in sys.stdin:
    message = json.loads(line)
    kind = message.get("type")
    if kind == "shutdown":
        break
    if kind == "initialize":
        continue
    if kind != "command":
        continue
    if message.get("arguments", "").strip() == "clear":
        status = {"key": "model", "segments": []}
    else:
        provider = os.environ.get("NIMINAL_PROVIDER", "provider")
        model = os.environ.get("NIMINAL_MODEL", "model")
        status = {"key": "model", "segments": [
            {"text": " niminal ", "style": "emphasis"},
            {"text": provider, "style": "accent"},
            {"text": " / ", "style": "muted"},
            {"text": model, "style": "success"},
        ]}
    send({"type": "response", "id": message["id"], "status": status,
          "message": "Footer status updated."})
