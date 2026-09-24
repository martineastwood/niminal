#!/usr/bin/env python3
import json
import sys


def send(value):
    print(json.dumps(value), flush=True)


def host(method, **fields):
    send({"type": "host_request", "id": method, "method": method, **fields})
    for line in sys.stdin:
        response = json.loads(line)
        if response.get("type") == "host_response" and response.get("id") == method:
            return response
    return {"cancelled": True}


def ui(method, **fields):
    send({"type": "ui_request", "id": method, "method": method, **fields})
    for line in sys.stdin:
        response = json.loads(line)
        if response.get("type") == "ui_response" and response.get("id") == method:
            return response
    return {"cancelled": True}


send({"type": "register", "commands": [{"name": "host", "description": "Host requests"}]})

for line in sys.stdin:
    message = json.loads(line)
    if message.get("type") == "shutdown":
        break
    if message.get("type") != "command":
        continue
    payload = {}
    if message.get("arguments") == "model":
        payload["model"] = host("model.complete", system_prompt="Summarize",
                                prompt="history", max_tokens=123)
    elif message.get("arguments") == "ui":
        payload["question"] = ui("question", prompt="Pick one",
                                 options=["Red", "Blue"])
        payload["confirm"] = ui("confirm", prompt="Continue?")
        payload["input"] = ui("input", prompt="Branch?")
        payload["password"] = ui("password", prompt="Token?")
    else:
        payload["info"] = host("session.info")
        payload["name"] = host("session.name", name="handoff source")
        payload["usage"] = host("context.usage")
        payload["editor"] = host("ui.editor", title="Edit handoff", text="draft")
    send({"type": "response", "id": message.get("id"), "payload": payload})
