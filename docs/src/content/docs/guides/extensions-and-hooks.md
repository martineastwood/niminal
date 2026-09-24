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

## Add an OpenAI-compatible provider

An extension can add a provider that uses the OpenAI Chat Completions API. Register
its endpoint, default model, optional model suggestions, and API key environment
variables:

```python
send({
    "type": "register",
    "commands": [],
    "providers": [{
        "api": "openai-chat-completions",
        "name": "litellm",
        "api_url": "http://localhost:4000/v1/chat/completions",
        "default_model": "openai/gpt-4o-mini",
        "models": ["openai/gpt-4o-mini", "anthropic/claude-sonnet"],
        "api_key_env": ["LITELLM_API_KEY"]
    }]
})
```

After restarting Niminal or running `/reload`, select it with `/provider litellm`
or `--provider litellm`. Use `/model` to choose one of the registered models.
When your proxy does not require authentication, set `"requires_api_key": false`.
You can also register `url_match` and the `session_routing`, `stream_usage`,
`apply_cache`, and `prompt_cache_key` flags. Provider hooks can further change
request headers and JSON payloads.

This registration format currently supports OpenAI Chat Completions endpoints.
It does not add a new streaming protocol such as Anthropic Messages or Google
Generative AI.

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

## Shutting down

Niminal sends `{"type": "shutdown"}` when it quits, reloads your extension, or
switches sessions, and closes your stdin at the same time. Treat both as the same
signal: stop what you started and exit. Niminal waits three seconds for your
process to exit, then kills the process group it started you in, so anything you
leave running is orphaned. Keep the children you start in that group and stop
them yourself in the same pass:

```python
import json, subprocess, sys

child = subprocess.Popen(["npm", "run", "dev"])

def stop_child():
    child.terminate()
    try:
        child.wait(timeout=2)
    except subprocess.TimeoutExpired:
        child.kill()
        child.wait()

for line in sys.stdin:            # ends at EOF when Niminal closes stdin
    message = json.loads(line)
    if message["type"] == "shutdown":
        break
stop_child()
```

A child in its own session, started with `setsid` or Node's `detached: true`, is
outside that group: only your extension can stop it, so kill its process group
before you exit.

## Side effects

Responses and unsolicited `update` messages can:

- set footer `status`
- display a `widget` or `notification`
- persist an `entry` in the session
- queue a `user_message` with `deliver_as`: `now`, `steer`, or `follow_up`

Extension tools use the normal permission prompt unless their capabilities are
read-only (`read` and optional `user` only).

## Status and widgets

You can add styled segments to the footer or show a small widget above the
composer. Return either from a command, hook, or unsolicited `update` message:

```python
send({"type": "update",
      "status": {"key": "build", "segments": [
          {"text": "build ", "style": "muted"},
          {"text": "passing", "style": "success"}]},
      "widget": {"key": "tasks", "title": "Tasks", "content": [
          {"type": "list", "items": [
              {"text": "Implement the feature", "state": "active"},
              {"text": "Run the tests", "state": "pending"}]},
          {"type": "progress", "label": "Complete", "value": 1, "max": 2}],
        "actions": [{"id": "complete_next", "label": "Complete next"}]}})
```

Status styles are `normal`, `muted`, `accent`, `success`, `warning`, `error`,
and `emphasis`. Send an empty `segments` array with the same key to clear that
status.

Widgets support `text`, `list`, and `progress` content. List items can use the
`pending`, `active`, or `done` state. Widgets appear above the composer. To run
an action, focus the empty composer and press Tab, move through actions with
the up and down keys, then press Enter. Escape closes the action selector. The
extension receives `{"type":"ui_action","widget":"tasks","action":"complete_next"}`
and can refresh the widget by sending another `update`. To remove a widget,
send its key with an empty title, content array, and actions array.

The [extensions_and_tools](https://github.com/martineastwood/extensions_and_tools)
repository includes runnable examples in `extensions/powerline_footer`,
`extensions/todo_widget`, and `extensions/subagent_panel`. Copy an example
directory under
`.niminal/extensions/` in a trusted workspace, then restart Niminal to load it.
The commands are `/footer_demo`, `/todos`, and `/subagents_demo`.

The todo example also registers a `todo` tool the agent can use to create,
update, list, inspect, delete, or clear tasks. For example, ask the agent to
"implement the settings screen and track the work in todos." It can mark a task
in progress as it starts and complete it when its work and checks are done.
Run `/todos` to review the current list. Tasks are saved under
`~/.niminal/todos/`, separately for each workspace and session, so they remain
available after an extension reload or context compaction. When every visible
task is complete, choose **Clear todos** in the widget to remove the list and
hide the widget. Because the tool changes saved tasks, Niminal asks for
permission before the agent uses it.

The subagent panel is a UI demo with simulated work items; it does not start
child agents.

Extensions can also ask Niminal to show a question, confirmation, input, password,
or external editor. Host requests support one-shot model completion, session
information and naming, and context usage.

## Limitations

Tool results currently add text parts to model context. Image tool-result parts
are ignored.

Built-in tool names (`read`, `grep`, `glob`, `ls`, `edit`, `write`, `bash`,
`skill`, `ask_user`) cannot be registered by extensions.

Extension errors fail open: a broken extension is skipped rather than stopping the
agent.

## Next steps

- [External tools](/guides/external-tools/) for one-shot executables
- [Context and compaction](/guides/context-and-compaction/) for compaction hooks
