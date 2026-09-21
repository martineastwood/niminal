---
title: Extensions and hooks
description: Persistent programs that register tools, commands, hooks, and live status over JSON lines.
---

Extensions are persistent programs that register tools, slash commands, and
lifecycle hooks over JSON lines on stdin and stdout.

## Where extensions live

Global extensions (always loaded; all matching extensions start):

```text
~/.agents/extensions/NAME/extension.json
~/.niminal/extensions/NAME/extension.json
```

Project extensions (trusted workspace only):

```text
<workspace>/.agents/extensions/NAME/extension.json
<workspace>/.niminal/extensions/NAME/extension.json
```

Unlike skills and external tools, extension names do not override each other.
Every discovered extension starts.

## Minimal extension

`extension.json`:

```json
{
  "name": "hello",
  "command": ["./extension.py"],
  "response_timeout_seconds": 30
}
```

`extension.py`:

```python
#!/usr/bin/env python3
import json, sys

def send(value):
    print(json.dumps(value), flush=True)

send({
    "type": "register",
    "commands": [{"name": "hello", "description": "Say hello"}],
    "tools": [{
        "name": "hello_tool",
        "description": "Return a greeting.",
        "input_schema": {"type": "object"},
        "capabilities": ["read"]
    }],
    "events": ["tool_call", "tool_result"]
})

for line in sys.stdin:
    message = json.loads(line)
    if message["type"] == "shutdown":
        break
    if message["type"] == "command":
        send({"type": "response", "id": message["id"],
              "message": "Hello " + message.get("arguments", "")})
    elif message["type"] == "tool":
        send({"type": "response", "id": message["id"],
              "content": [{"type": "text", "text": "Hello"}]})
    elif message["type"] == "event":
        send({"type": "response", "id": message["id"]})
```

The host sends `initialize` first. Your program replies with one `register`
object, then answers every `command`, `tool`, and `event` request with a
`response` carrying the same `id`. Stdout is reserved for protocol messages.

Use `/reload` after changing a manifest or extension program.

## Lifecycle hooks

Extensions can subscribe to:

| Event | Purpose |
| --- | --- |
| `tool_call` | Block or replace tool arguments before execution |
| `tool_result` | Replace tool output |
| `turn_start` | Run at the start of a user turn |
| `turn_end` | Run when a turn finishes |
| `context` | Append system strings or messages before the model request |
| `session_start` | Session opened |
| `session_end` | Session closed |
| `session_before_compact` | Block compaction or add an instruction |
| `session_compact` | Supply a custom compaction result |

## Side effects

Responses and unsolicited `update` messages can:

- set footer `status`
- display a `widget` or `notification`
- persist an `entry` in the session
- queue a `user_message` with `deliver_as`: `now`, `steer`, or `follow_up`

Extension tools use the normal permission prompt unless their capabilities are
read-only (`read` and optional `user` only).

## Limitations

The current host accepts text tool-result parts. UI and host requests are answered
as unavailable, and image result parts are not added to the model context yet.

Built-in tool names (`read`, `grep`, `glob`, `edit`, `write`, `bash`, `skill`)
cannot be registered by extensions.

Extension errors fail open: a broken extension is skipped rather than stopping the
agent.

## Next steps

- [External tools](/guides/external-tools/) for one-shot executables
- [Context and compaction](/guides/context-and-compaction/) for compaction hooks
