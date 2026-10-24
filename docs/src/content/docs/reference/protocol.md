---
title: Sequencer protocol
description: JSON-RPC 2.0 over WebSocket for live evaluation, clips and scenes, and the state of a live session.
---

**Read by nimlive.** This is the sequencer's API, not the server's: what a program uses to start clips, set tempo and make quantized changes.

`niminal server` is the generic audio server on port 7401: its API is the [Engine API](/reference/engine-api/). Clips, scenes, lanes, transforms, looping and quantized edits belong to the **live sequencer client**, which owns tempo, meter and musical position and turns them into events stamped in absolute samples.

VS Code starts and owns a private `nimlive live --port 0 --parent-stdio` process, connected to the configured server. Closing the editor stops its sequencer. For terminal use, start `nimlive live [FILE]`: its live-control API defaults to `ws://127.0.0.1:7400/`, with `--engine-port 7401` selecting the server. Each live process owns its own performance state. Other clients can use the generic engine directly without using these abstractions.

This page describes the live client's **JSON-RPC 2.0** protocol over **WebSocket**. It is separate from **`nimlive lsp`**, which speaks LSP on stdio for editing.

## Connection

1. Open a WebSocket to the server.
2. Send **`hello`** as the first request (within about three seconds, or the connection may time out).
3. If the server was started with **`--token`**, the first message must be `hello` and **`params.token`** must match. Until then, other requests get a protocol error.
4. Subscribe to topics if you want push updates (optional).
5. Send requests with an **`id`**; the server replies with the same **`id`**. Requests **without** an `id` are notifications and get **no reply** (used for `panic`).

Clients share a **chair**: the performance (clips, scenes, mute and solo, what is playing, pending changes) belongs to the chair, not to a connection, and disconnecting a client does not stop playback. A client sits in `main` unless its `hello` names another chair, so by default all clients share one session. Other chairs are seated on demand (up to eight), each over an engine connection of its own, so its definitions, buffers and note ids are private too. See [Chairs](#chairs). A program that wants to send timed events rather than code should talk to the [engine](/reference/engine-api/) instead, or as well.

Limits: up to **16** clients; maximum message size **64 MiB** (large `eval` sources, and the events of a long `render.start`).

## Handshake: `hello`

**Request**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "hello",
  "params": {
    "protocol": 5,
    "client": "my-app",
    "token": "optional-if-server-requires-it",
    "chair": "optional-name-of-a-chair"
  }
}
```

**Result**

```json
{
  "protocol": 5,
  "sample_rate": 48000,
  "channels": 2,
  "topics": ["landed", "pending", "transport", "meters", "notices", "learned"],
  "chair": "main"
}
```

**`protocol`** must be **`4`** (`PROTOCOL_VERSION` in the server). A mismatch returns error code **-32000**.

## Chairs

A chair is one player's seat in a shared room: a full sequencer with clips, scenes, mute, solo, quantized changes and a connection to the engine of its own. Name one in `hello` with `chair` and it is created if it is new; clients that name the same chair share it, as clients of one session always did. Everything a client does (`eval`, `launch`, `mute`, `status`, `cancel`, `hush`, `panic`) and every notification it receives is about its own chair. `hush` and `panic` silence only that chair, so a mistake on one chair does not stop the others.

- **Tempo.** A new chair starts at the tempo and meter that `main` has when the chair is created, and keeps its own from there. Bars of chairs at the same tempo and meter line up, because they count from the same sample. Chairs do not follow each other's later changes: the engine has no tempo, so one chair is the tempo authority by agreement, and the others evaluate the same `tempo` line when it changes.
- **Controls.** The keyboard, MIDI and OSC controls of `nimlive live` play in `main`.
- **Limits.** Up to eight chairs. A server that was not started with chairs (the in-process one used by `nimlive render`) has only `main` and refuses another name.

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
  "changes": ["lead.reverse"]
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
- **`scenes`**, **`defined_clips`** — defined names
- **`clips`** — `name`, `clip`, `muted`, `soloed`

### Performance API

Typed calls that compile to the same `.nml` source as `eval`, so quantize, the session log and replay all work unchanged. Every method takes an optional **`quantize`**.

| Method | Params | Source form |
| --- | --- | --- |
| `clip.define` | a [ClipDef](/reference/score-model/) (`name`, optional `instrument`, `length`, `loop`, `lanes`, `events`) | `clip name { … }` |
| `scene.define` | a SceneDef (`name`, `clips`: a list of clip names) | `scene name { … }` |
| `launch` | `{ "target": "verse" }`: a scene, a clip or an arrangement | `verse.start` |
| `stop` / `mute` / `unmute` / `solo` | `{ "target": "bass" }`: a clip, or a scene for all its clips | `bass.stop` / `.mute` / … |
| `unsolo` | optional `target` | `bass.unsolo` or `unsolo` |
| `edit` | `{ "clip": "bass", "change": "reverse" }` | `bass.reverse` (`undo`, `clear`, `undo(reverse)` also work) |

`status` includes each clip's `playing` base, `edits` stack, `clip`, `muted` and `soloed`, plus the defined `scenes` and `defined_clips`.

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
→ {"jsonrpc":"2.0","id":1,"method":"hello","params":{"protocol":5,"client":"example"}}
← {"jsonrpc":"2.0","id":1,"result":{"protocol":5,"sample_rate":48000,"channels":2,"topics":[...]}}

→ {"jsonrpc":"2.0","id":2,"method":"subscribe","params":{"topics":["landed","pending","transport"]}}
← {"jsonrpc":"2.0","id":2,"result":{"topics":["landed","pending","transport"]}}

→ {"jsonrpc":"2.0","id":3,"method":"eval","params":{"source":"tempo 120bpm\ninstr x(freq: hz) { osc(sine, freq) }\nclip t { instrument: x, notes: [c4 e4 g4] }\nt.start","quantize":"now"}}
← {"jsonrpc":"2.0","id":3,"result":{"id":1,"lands_at":0,...}}

→ {"jsonrpc":"2.0","id":4,"method":"eval","params":{"source":"t.transpose(+12st)","quantize":"next bar"}}
← {"jsonrpc":"2.0","id":4,"result":{"id":2,"lands_at":96000,"position":{"bar":2,"beat":1.0},...}}

← {"jsonrpc":"2.0","method":"landed","params":{"id":2,"at":96000,"changes":["t.transpose(+12st)"],...}}
```

See [Command line](/reference/nimlive/cli/) (`send`, `repl`), [Editor and LSP](/reference/editor/), and [Live coding](/guide/live-coding/).
