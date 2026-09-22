---
title: RPC mode
description: Drive a long-running niminal process with JSON commands on stdin.
---

RPC mode keeps niminal running and accepts one JSON command per stdin line. Use it
when an outer program needs prompts, steering, follow-ups, interrupts, and session
state across many turns.

```sh
niminal --mode rpc
```

Every response is a versioned JSON line:

```json
{"version":1,"type":"response","id":"1","ok":true,...}
```

Turn progress uses the same JSONL event types as [JSON mode](/reference/json-mode/),
plus `queue` events when steering or follow-up queues change. EOF or Ctrl+C
requests shutdown. Pass `--no-session` to keep the transcript in memory only.
RPC mode does not accept a CLI prompt; send commands on stdin instead.

## Commands

| Command | Purpose |
| --- | --- |
| `prompt` | Start a turn. Requires `"message"`. While busy, also requires `"streamingBehavior": "steer"` or `"followUp"` |
| `steer` | Queue a steering message while busy |
| `follow_up` | Queue a follow-up message while busy |
| `get_state` | Session id, busy flag, queue depths, queue modes, `"mode": "act"` |
| `clear_queue` | Drain steering and follow-up queues (response includes the removed messages) |
| `set_steering_mode` | `"mode": "all"` or `"one-at-a-time"` (saved to config) |
| `set_follow_up_mode` | Same values as steering |
| `interrupt` | Cancel the running turn |
| `shutdown` | Stop gracefully |

Example:

```sh
printf '%s\n' \
  '{"id":"1","type":"get_state"}' \
  '{"id":"2","type":"prompt","message":"Summarize this repo in one paragraph."}' \
  '{"id":"3","type":"shutdown"}' \
  | niminal --mode rpc
```

To attach a workspace image, include an `@` reference in the message:

```json
{"id":"4","type":"prompt","message":"Review @screenshots/home.png"}
```

PNG, JPEG, and WebP images up to 10 MiB are supported. You can also send an
explicit image file path as the whole message, including an absolute path outside
the workspace. The selected model must support image input.

## Queue modes

`steering_mode` and `follow_up_mode` control whether queued messages are delivered
one at a time or all at once. Defaults are `one-at-a-time`. RPC commands that
change them write to `~/.niminal/config.json`.

`get_state` returns:

```json
{
  "version": 1,
  "type": "response",
  "id": "1",
  "ok": true,
  "session_id": "...",
  "busy": false,
  "queued": false,
  "steering": 0,
  "follow_up": 0,
  "steering_mode": "one-at-a-time",
  "follow_up_mode": "one-at-a-time",
  "mode": "act"
}
```

There is no plan mode toggle. niminal always reports `"mode": "act"`.

Headless RPC has no approval UI, so tools run without prompting.

Successful command responses use `"state": "started"` or `"queued"`. Other
states include `interrupting`, `idle`, `stopped`, and `stopping`. Queue events
use `"mode": "steer"` or `"follow_up"` with actions `enqueue`, `dequeue`, or
`clear`.

## Next steps

- [JSON mode](/reference/json-mode/) for event types during a turn
- [Configuration](/guides/configuration/) for saved queue modes
