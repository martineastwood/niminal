#!/usr/bin/env python3
import json
import sys


def send(value):
    print(json.dumps(value), flush=True)


json.loads(sys.stdin.readline())
send({"type": "register", "commands": [{"name": "parallel", "description": "Parallel"}]})
first = json.loads(sys.stdin.readline())
second = json.loads(sys.stdin.readline())


def result(message):
    return "first" if message.get("arguments") == "one" else "second"


send({"type": "response", "id": second.get("id"), "message": result(second)})
send({"type": "response", "id": first.get("id"), "message": result(first)})

for line in sys.stdin:
    if json.loads(line).get("type") == "shutdown":
        break
