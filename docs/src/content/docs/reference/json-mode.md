---
title: JSON mode
description: Emit versioned JSONL events for a single run.
---

JSON mode runs one turn, prints versioned JSONL events to stdout, and exits.
Use it when another program drives niminal for a single request.

```sh
niminal --mode json "Explain what this repo does."
cat README.md | niminal --mode json "Summarize this"
```

To attach a workspace image, include an `@` reference in the prompt:

```sh
niminal --mode json "Review @screenshots/home.png"
```

PNG, JPEG, and WebP images up to 10 MiB are supported. You can also send an
explicit image file path as the whole prompt. The selected model must support
image input.

Diagnostics and startup errors go to stderr. The process exits `0` on success and
`1` when the turn failed.

## Bookends

Every stream starts and ends with session events:

```json
{"version":1,"type":"session_start","session_id":"..."}
{"version":1,"type":"session_end","session_id":"...","success":true}
```

## Event types

All records include `"version":1`. Many stream events also include `step`,
`model`, and `duration_ms` when applicable.

| Type | When |
| --- | --- |
| `message` | Complete user or assistant message (`role`, `content`, optional `final`) |
| `run_start` | Turn begins (`prompt` on the first event) |
| `run_end` | Turn finished |
| `step_start` | Model step begins (`step`, optional `model`) |
| `step_end` | Model step ends (`step`, optional `usage`) |
| `message_delta` | Streamed assistant text (`delta`) |
| `thinking_delta` | Streamed thinking text (`delta`) |
| `tool_call` | Model requested a tool (`tool_id`, `tool_name`, optional `input`) |
| `tool_output_delta` | Streamed bash output so far (`tool_id`, `tool_name`, `delta` is the captured snapshot, not an append-only chunk) |
| `tool_result` | Tool finished (`output`, `is_error`) |
| `error` | Failure (`message`) |

`queue` events are emitted in [RPC mode](/reference/rpc-mode/), not in
`--mode json` runs.

`status` and `done` events are not written to stdout in JSON mode.

`approval_required` exists in the schema but is not emitted in headless modes
because tools run without prompting there.

Optional identity fields on many events: `session_id`, `turn_id`, `run_id`.

`step_end` may include:

```json
"usage": {
  "input_tokens": 1200,
  "output_tokens": 400,
  "cache_read_tokens": 0,
  "cache_write_tokens": 0,
  "cache_reported": false
}
```

Assistant `message` events include a `final` boolean. Intermediate tool-step
messages use `final: false`; the closing message for a step uses `final: true`.

At turn start, JSON mode emits both `run_start` (with `prompt`) and a separate
user `message` event with the same content.

## Next steps

- [RPC mode](/reference/rpc-mode/) for a long-running process
- [Commands and shortcuts](/reference/commands/#cli-flags) for flags
