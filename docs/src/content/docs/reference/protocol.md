---
title: Daemon protocol
description: JSON-RPC 2.0 over WebSocket for live evaluation, streaming events, and session state.
---

The **daemon** (`niminal daemon`) exposes a single session to multiple clients over **WebSocket**. Messages are **JSON-RPC 2.0** text frames. This is separate from **[`niminal lsp`](/reference/editor/)**, which speaks the Language Server Protocol on stdio for editing only.

Default URL: `ws://127.0.0.1:7400/` (see `--port` and `--listen` on the daemon).

## Connection

1. Open a WebSocket to the daemon.
2. Send **`hello`** as the first request (within about three seconds, or the connection may time out).
3. If the daemon was started with **`--token`**, the first message must be `hello` and **`params.token`** must match. Until then, other requests get a protocol error.
4. Subscribe to topics if you want push updates (optional).
5. Send requests with an **`id`**; the daemon replies with the same **`id`**. Requests **without** an `id` are notifications and get **no reply** (used for `panic`).

All connected clients share **one session**. Disconnecting a client does not stop playback.

Limits: up to **16** clients; maximum message size **4 MiB** (large `eval` sources).

## Handshake: `hello`

**Request**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "hello",
  "params": {
    "protocol": 1,
    "client": "my-app",
    "token": "optional-if-daemon-requires-it"
  }
}
```

**Result**

```json
{
  "protocol": 1,
  "sample_rate": 48000,
  "channels": 2,
  "topics": ["landed", "pending", "transport", "meters", "notices", "learned"]
}
```

**`protocol`** must be **`1`** (`PROTOCOL_VERSION` in the daemon). A mismatch returns error code **-32000**.

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
- **`realtime`** — optional counters (`late_events`, `starved_frames`, `capacity_drops`, `control_drops`, `deadline_misses`, `max_render_micros`, …)
- **`controls`** — each `ctl`: `name`, `value`, `target`, `unit` (db shown in dB)
- **`scenes`**, **`clips`** — defined names
- **`tracks`** — `name`, `clip`, `muted`, `soloed`

### `events.send`

One batched method for every timed thing a client can do. The whole batch is checked before any of it is queued.

```json
{ "method": "events.send", "params": { "events": [
  { "op": "note", "target": "lead", "at": "64beat", "dur": "1/2beat", "args": { "freq": "c4" } },
  { "op": "control", "name": "cutoff", "at": 96000, "value": 2, "unit": "khz", "glide": "50ms" },
  { "op": "track", "name": "pad", "at": "now", "args": { "cut": "800hz" } }
] } }
```

**`at`** is a whole number (an absolute sample), `"now"` (as soon as possible) or a time with a unit (`"32beat"`, `"8.5bar"`, `"12.25sec"`): a position on the transport, measured from its bar origin. `dur` and `glide` are durations (`"1/2beat"`, `"250ms"`).

| `op` | Fields | Meaning |
| --- | --- | --- |
| `note` | `target`, `at`, `dur`, optional `args` | Play a note on a track or instrument. `args` are named parameters as in [Score model](/reference/score-model/). |
| `control` | `name`, `at`, `value`, optional `unit`, `glide` | Move a `ctl`. `unit` defaults to the declaration's unit; compatible units are converted. `glide` defaults to the control's own smoothing. |
| `track` | `name`, `at`, `args` | Set parameters of a track (what a clip lane like `cut:` sets). |

**Result:** `{ "accepted": 3, "late": 0, "first": 4800, "horizon": 8192 }`. `late` counts events whose time had already passed: they sounded at once. `first` is the earliest sample any event resolved to, and `horizon` is the sample up to which the engine has prepared audio.

Anything wrong in the batch → **-32001** and nothing is queued; each entry of **`problems`** has an **`event`** field with the index of the event it is about. A batch that doesn't parse → **-32602**.

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

**Problem object:** `message`, `help`, `line`, `column`, `span` (`start`, `end`), optional `in_definition`, and for `events.send` the `event` index.

## Evaluation log and replay

With default logging, the daemon writes session input under **`~/.niminal/sessions/`**. Replay offline:

```sh
niminal render --replay ~/.niminal/sessions/<log> --out set.wav --channels 2
```

Evaluations, **`events.send`**, MIDI/OSC (when enabled), and related control input are recorded so replay matches live audio.

## Planned (spec, not in the daemon yet)

The [language spec](https://github.com/martineastwood/niminal/blob/main/niminal%20Spec.md) also describes **`event.update`**, **`file.put`**, remote TLS, and richer meter streaming. Those are not implemented on the wire today. Clips and scenes are defined with **`clip.define`** / **`scene.define`** (or `eval`), and timed events with **`events.send`**.

## Minimal client flow

```json
→ {"jsonrpc":"2.0","id":1,"method":"hello","params":{"protocol":1,"client":"example"}}
← {"jsonrpc":"2.0","id":1,"result":{"protocol":1,"sample_rate":48000,"channels":2,"topics":[...]}}

→ {"jsonrpc":"2.0","id":2,"method":"subscribe","params":{"topics":["landed","pending","transport"]}}
← {"jsonrpc":"2.0","id":2,"result":{"topics":["landed","pending","transport"]}}

→ {"jsonrpc":"2.0","id":3,"method":"eval","params":{"source":"tempo 120bpm\ninstr x(freq: hz) { osc(sine, freq) }\ntrack t { instrument = x }","quantize":"now"}}
← {"jsonrpc":"2.0","id":3,"result":{"id":1,"lands_at":0,...}}

→ {"jsonrpc":"2.0","id":4,"method":"eval","params":{"source":"t.play([c4 e4 g4])","quantize":"next bar"}}
← {"jsonrpc":"2.0","id":4,"result":{"id":2,"lands_at":96000,"position":{"bar":2,"beat":1.0},...}}

← {"jsonrpc":"2.0","method":"landed","params":{"id":2,"at":96000,"changes":["t.play([c4 e4 g4])"],...}}
```

See also [Command line](/reference/cli/) (`send`, `repl`), [Editor and LSP](/reference/editor/), and [Live coding](/guide/live-coding/).
