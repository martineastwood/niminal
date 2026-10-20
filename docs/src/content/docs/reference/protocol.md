---
title: Sequencer protocol
description: JSON-RPC 2.0 over WebSocket for live evaluation, clips and scenes, and the state of a live session.
---

`niminal server` is the generic audio server on port 7401: its API is the [Engine API](/reference/engine-api/). Clips, scenes, lanes, transforms, looping and quantized edits belong to the **live sequencer client**, which owns tempo, meter and musical position and turns them into events stamped in absolute samples.

VS Code starts and owns a private `nimlive live --port 0 --parent-stdio` process, connected to the configured server. Closing the editor stops its sequencer. For terminal use, start `nimlive live [FILE]`: its live-control API defaults to `ws://127.0.0.1:7400/`, with `--engine-port 7401` selecting the server. Each live process owns its own performance state. Other clients can use the generic engine directly without using these abstractions.

This page describes the live client's **JSON-RPC 2.0** protocol over **WebSocket**. It is separate from **`nimlive lsp`**, which speaks LSP on stdio for editing.

## Connection

1. Open a WebSocket to the server.
2. Send **`hello`** as the first request (within about three seconds, or the connection may time out).
3. If the server was started with **`--token`**, the first message must be `hello` and **`params.token`** must match. Until then, other requests get a protocol error.
4. Subscribe to topics if you want push updates (optional).
5. Send requests with an **`id`**; the server replies with the same **`id`**. Requests **without** an `id` are notifications and get **no reply** (used for `panic`).

All connected clients share **one session**. Disconnecting a client does not stop playback. A program that wants to send timed events rather than code should talk to the [engine](/reference/engine-api/) instead, or as well.

Limits: up to **16** clients; maximum message size **4 MiB** (large `eval` sources).

## Handshake: `hello`

**Request**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "hello",
  "params": {
    "protocol": 4,
    "client": "my-app",
    "token": "optional-if-server-requires-it"
  }
}
```

**Result**

```json
{
  "protocol": 4,
  "sample_rate": 48000,
  "channels": 2,
  "topics": ["landed", "pending", "transport", "meters", "notices", "learned"]
}
```

**`protocol`** must be **`4`** (`PROTOCOL_VERSION` in the server). A mismatch returns error code **-32000**.

## Subscriptions

**`subscribe`** / **`unsubscribe`** — params: `{ "topics": ["landed", "pending", ...] }`.

Valid topics (same as in `hello`):

| Topic | Notification `method` | When |
| --- | --- | --- |
| `landed` | `landed` | A queued change reached its quantize boundary |
| `pending` | `pending` | The set of waiting changes changed |
| `transport` | `transport` | About ten times per second while the clock runs |
| `meters` | `meters` | With `transport`, if anyone subscribed (peak readings) |
| `notices` | `notice` | Warnings (playback issues, cancelled dependents, NaN voices, …) |
| `learned` | `learned` | A `midi.learn` control took a control change: `{ "control": "filth", "source": "midi.cc(21)" }` |

Notifications have **no `id`**:

```json
{ "jsonrpc": "2.0", "method": "landed", "params": { ... } }
```

## Requests

### `eval`

Compile and queue niminal source. Failed compile returns **-32001** with **`error.data.problems`** (same shape as editor diagnostics).

**Params**

| Field | Type | Description |
| --- | --- | --- |
| `source` | string | **Required.** One evaluation unit (block, file, or line). |
| `quantize` | string | When it lands: `now`, `beat`, `bar`, `next bar`, `next 4bars`, `cycle`, etc. Omitted uses session defaults. |
| `dir` | string | Optional sample search path for this eval (project-relative). |

**Result** (accepted)

```json
{
  "id": 2,
  "lands_at": 96000,
  "position": { "bar": 2, "beat": 1.0 },
  "in_seconds": 1.79,
  "changes": ["lead.play([c4])"]
}
```

- **`id`** — use with **`cancel`** to drop this change before it lands.
- **`lands_at`** — sample time on the session clock.
- **`changes`** — human-readable summary of what will apply.

Compilation runs **off the audio thread**; the server may compile in a worker thread and retry if the pending queue changes mid-compile.

### `cancel`

Drop pending changes before they land.

**Params:** `{ "id": 2 }` — cancel that change (and dependents). Omit **`id`** to cancel one pending change (implementation-defined which when several are queued).

**Result:** `{ "cancelled": 1 }` — number removed.

### `hush`

Graceful stop: clips stop, notes can ring out. Queued like **`eval`** (returns the same **`Accepted`** shape as a successful eval).

### `panic`

Immediate silence: stops voices, clears delay lines within one engine block. **Result:** `{}`.

Can be sent as a notification (no `id`) for an instant stop with no JSON reply.

### `status`

Snapshot of the session (no subscription needed).

**Result** includes:

- **`transport`** — `sample`, `seconds`, `bar`, `beat`, `bpm`, `voices`, `pending` (count)
- **`pending`** — array of pending changes (same fields as `pending` notifications)
- **`sample_rate`**, **`channels`**
- **`realtime`**, **`controls`**, **`instruments`** — as the [engine reports them](/reference/engine-api/#status)
- **`scenes`**, **`clips`** — defined names
- **`tracks`** — `name`, `clip`, `muted`, `soloed`

### Performance API

Typed calls that compile to the same `.nml` source as `eval`, so quantize, the session log and replay all work unchanged. Every method takes an optional **`quantize`**.

| Method | Params | Source form |
| --- | --- | --- |
| `clip.define` | a [ClipDef](/reference/score-model/) (`name`, optional `instrument`, `length`, `loop`, `lanes`, `events`) | `clip name { … }` |
| `scene.define` | a SceneDef (`name`, `entries`) | `scene name { … }` |
| `launch` | `{ "target": "verse" }` | `verse.start` |
| `stop` / `mute` / `unmute` / `solo` | `{ "target": "bass" }` | `bass.stop` / `.mute` / … |
| `unsolo` | optional `target` | `bass.unsolo` or `unsolo` |
| `edit` | `{ "track": "bass", "change": "reverse" }` | `bass.reverse` (`undo`, `clear`, `undo(reverse)` also work) |

`status` includes each track's `playing` base, `edits` stack, `clip`, `muted` and `soloed`, plus the defined `scenes` and `clips`.

## Error codes

| Code | Meaning |
| --- | --- |
| -32700 | Parse error (invalid JSON) |
| -32600 | Invalid request (missing `jsonrpc`, `method`, …) |
| -32601 | Method not found |
| -32602 | Invalid params |
| -32603 | Internal error (session continues) |
| -32000 | Protocol error (hello, version, token) |
| -32001 | Rejected — compile/validation failed; see **`data.problems`** |

**Problem object:** `message`, `help`, `line`, `column`, `span` (`start`, `end`), optional `in_definition`.

## Logs and replay

With default logging, the server writes what the **engine** was sent under **`~/.niminal/sessions/`**: every definition, note, parameter change and cancellation, with the samples they were stamped for. That is all it takes to play a live set again, whatever the sequencer was doing. Replay offline:

```sh
niminal replay ~/.niminal/sessions/<log> --out set.wav --channels 2
```

## Planned (spec, not in the server yet)

The [language spec](https://github.com/martineastwood/niminal/blob/main/niminal%20Spec.md) also describes **`event.update`**, **`file.put`**, remote TLS, and richer meter streaming. Those are not implemented on the wire today. Clips and scenes are defined with **`clip.define`** / **`scene.define`** (or `eval`).

## Minimal client flow

```json
→ {"jsonrpc":"2.0","id":1,"method":"hello","params":{"protocol":4,"client":"example"}}
← {"jsonrpc":"2.0","id":1,"result":{"protocol":4,"sample_rate":48000,"channels":2,"topics":[...]}}

→ {"jsonrpc":"2.0","id":2,"method":"subscribe","params":{"topics":["landed","pending","transport"]}}
← {"jsonrpc":"2.0","id":2,"result":{"topics":["landed","pending","transport"]}}

→ {"jsonrpc":"2.0","id":3,"method":"eval","params":{"source":"tempo 120bpm\ninstr x(freq: hz) { osc(sine, freq) }\ntrack t { instrument = x }","quantize":"now"}}
← {"jsonrpc":"2.0","id":3,"result":{"id":1,"lands_at":0,...}}

→ {"jsonrpc":"2.0","id":4,"method":"eval","params":{"source":"t.play([c4 e4 g4])","quantize":"next bar"}}
← {"jsonrpc":"2.0","id":4,"result":{"id":2,"lands_at":96000,"position":{"bar":2,"beat":1.0},...}}

← {"jsonrpc":"2.0","method":"landed","params":{"id":2,"at":96000,"changes":["t.play([c4 e4 g4])"],...}}
```

See also [Command line](/reference/cli/) (`send`, `repl`), [Editor and LSP](/reference/editor/), and [Live coding](/guide/live-coding/).
