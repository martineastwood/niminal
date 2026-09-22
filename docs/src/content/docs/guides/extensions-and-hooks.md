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
| `input` | Replace submitted text or return `allow: false` before it is saved |
| `before_agent_start` | Set `system_prompt` for this run or add a persistent `message` |
| `message_end` | Replace the completed assistant text before it is saved |
| `agent_settled` | Observe when the run has finished, including queued follow-ups |
| `session_before_switch` | Return `allow: false` to cancel a TUI session change |
| `session_shutdown` | Clean up before quitting, reloading, or changing sessions |
| `before_provider_request` | Replace the provider JSON `payload` before sending it |
| `before_provider_headers` | Add, replace, or remove HTTP `headers` |
| `after_provider_response` | Observe HTTP `status`, `headers`, `duration_ms`, and `error_body` |
| `session_compact_failed` | Observe a model error or empty compaction summary |

For example, you can tailor one run without changing the user's saved prompt.
Register `before_agent_start` in your extension's `events` list, then reply:

```python
elif message["type"] == "event" and message["event"] == "before_agent_start":
    prompt = message["payload"]["prompt"]
    send({"type": "response", "id": message["id"],
          "system_prompt": "Answer briefly and verify changes.",
          "message": {"content": f"Task: {prompt}"}})
```

`system_prompt` replaces the base system text for this run. Project instructions
and other appended system text still apply. The `message` is saved in the session
and sent as a user message to the model. Omit either field if you do not need it.

`input` receives `text` and `images`; return `text` to replace the submitted
text, or `{"allow": false, "reason": "..."}` to stop the turn. Provider hooks
receive the selected `provider`, `model`, and session ID. For
`before_provider_headers`, return a `headers` object with string values to set
headers and `null` to remove them. For `before_provider_request`, return a
complete `payload` object to replace the outgoing JSON body.

`session_before_switch` runs for new, resumed, and forked sessions in the TUI.
`session_shutdown` includes `reason`: `quit`, `reload`, `new`, `resume`, or
`fork`. `after_provider_response` sends an empty `error_body` on successful
responses. On HTTP errors, it contains the response body.

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

Built-in tool names (`read`, `grep`, `glob`, `ls`, `edit`, `write`, `bash`, `skill`)
cannot be registered by extensions.

Extension errors fail open: a broken extension is skipped rather than stopping the
agent.

## Next steps

- [External tools](/guides/external-tools/) for one-shot executables
- [Context and compaction](/guides/context-and-compaction/) for compaction hooks
