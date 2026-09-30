#!/usr/bin/env python3
import json
import sys

line = sys.stdin.readline()
if not line:
    sys.exit(1)
message = json.loads(line)
if message.get("type") != "initialize":
    sys.exit(2)
if not isinstance(message.get("trusted"), bool):
    sys.exit(3)

trusted = message["trusted"]

def send(value):
    print(json.dumps(value), flush=True)

send({
    "type": "register",
    "commands": [{"name": "trusted_echo", "description": "Echo initialize trusted flag"}],
    "tools": [],
    "events": [],
})

for line in sys.stdin:
    message = json.loads(line)
    if message.get("type") == "shutdown":
        break
    if message.get("type") == "command":
        send({"type": "response", "id": message["id"],
              "message": "trusted=" + ("true" if trusted else "false")})
