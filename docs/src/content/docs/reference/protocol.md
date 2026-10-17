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
- **`controls`** — each `ctl` and track parameter (named `track::param`): `name`, `value`, `target`, `unit` (db shown in dB)
- **`instruments`** — each instrument's `name` and `params` (`name`, `unit`, `default`, `range`)
- **`buses`** — bus names
- **`clients`** — each connected client that has something going: `id`, `voices` (the ids of its held notes) and `waiting` (how many of its events are still waiting)
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
| `note` | `target`, `at`, optional `args`, `dur`, `id`, `tag` | Play a note on a track or instrument. `args` are named parameters as in [Score model](/reference/score-model/). With `dur` and no `id` it is fire-and-forget. With an `id` and no `dur` it is held until an `off`. With both it is held for at most `dur`. |
| `set` | `id`, `at`, `args`, optional `glide` | Change parameters of a sounding note, or of one that hasn't started yet (it then starts with the value). Any declared parameter will do. With a `glide` the parameter moves in a straight line to its new value. |
| `off` | `id`, `at` | Release a note, so its envelope runs its release stage. An `off` before the note starts makes it a short blip. |
| `control` | `name`, `at`, `value`, optional `unit`, `glide` | Move a `ctl`. `unit` defaults to the declaration's unit; compatible units are converted. `glide` defaults to the control's own smoothing. |
| `track` | `name`, `at`, `args`, optional `glide` | Set parameters of a track (what a clip lane like `cut:` sets). `glide` defaults to the parameter's own smoothing. Planned into prepared audio like notes, so a lane can move a parameter many times a second. |

An **`id`** is a whole number the client picks, and it belongs to that connection: another client can use the same one. Using an id that is still sounding, or one that was never opened or has been let go of, fails the batch. Because the client picks it, a `note`, a `set` and an `off` can go in one batch with no round trip. When a client disconnects its held notes are released and its notes still waiting never sound.

| `define` | `source`, `at`, optional `dir` | Compile definitions (`instr`, `track`, effects, `bus`, `ctl`, `config`, tables, buffers, `fn`, `const`, samples) and swap them in at `at`, sample-exactly: the new mixer is built ahead and running notes carry over. Performance statements, clips, scenes and `tempo`/`meter` are an error. Compile problems come back like `eval`'s. `dir` is where sample files and `use` modules are looked for. |
| `transport` | `at`, any of `bpm`, `meter` (as `"3/4"`), `origin` | Change the tempo or meter, or say that this point is bar 1 (`"origin": true`). A ramp is several of these. |
| `forget` | `at` | Drop every definition, as `new` does. The output layout is kept. |

**Time and tempo.** The engine owns time: the tempo, the meter and the bar origin live in its transport, so every client resolves `"8bar"` or `"32beat"` the same way. An event given in beats or bars keeps its musical position if the tempo or meter changes before it plays: still-waiting notes, `define`s and the like are moved when the change lands. Events given in samples or seconds never move. `transport` changes themselves keep the time they were given.

Notes, controls and the rest in a batch are checked against the definitions in force at their own time, so a batch can `define` an instrument and play it. If anything in a batch is wrong, a `define` or `transport` queued earlier in the same batch is taken back.

**Result:** `{ "accepted": 3, "late": 0, "first": 4800, "horizon": 8192 }`. `late` counts events whose time had already passed: they sounded at once. `first` is the earliest sample any event resolved to, and `horizon` is the sample up to which the engine has prepared audio.

Anything wrong in the batch → **-32001** and nothing is queued; each entry of **`problems`** has an **`event`** field with the index of the event it is about. A batch that doesn't parse → **-32602**.

### `clock`

Where the engine is in time, for a client that schedules against it.

**Result**

| Field | Meaning |
| --- | --- |
| `sample` | The sample the output is at, as the audio callback last said. |
| `bar`, `beat` | The musical position of `sample`; `beat` counts from 1 in the bar, fractions included. |
| `bpm`, `beats_per_bar` | The tempo and meter in force at `sample`. |
| `sample_rate` | |
| `prepared_until` | The sample up to which audio has been prepared. An event stamped before it is put into prepared audio; later ones are planned when their turn comes. |
| `lookahead` | How far ahead of the clock the engine prepares, in samples. |
| `output_latency` | Seconds from the audio callback to the speakers, as the audio API reports it. |
| `wall_micros` | The Unix time in microseconds at which the audio callback was handed the buffer that starts at `sample`, recorded in the callback beside the clock rather than when the reply is made. `null` before there is an output. With `sample_rate` it maps any sample to the wall clock: `wall_micros + (s - sample) / sample_rate * 1e6`, plus `output_latency` to when it is heard. |

### `events.cancel`

Take back this client's events that are still waiting.

```json
{ "method": "events.cancel", "params": { "tag": "lead", "from": "64beat", "release": true } }
```

Any event in an `events.send` batch can carry a **`tag`**, a string of the client's choosing. `events.cancel` drops the waiting events (notes, `set`s, `off`s, `control` and `track` changes) with that tag that are at or after **`from`** (a time as for `at`; default `"now"`). Without `tag` it drops all of this client's waiting events. Other clients' events are never touched. With **`release`: true**, held notes with that tag that are already sounding are also let go of at `from`. A held note whose `off` was taken back is held again.

**Result:** `{ "cancelled": 12, "released": 1 }`

A control change that has already reached the audio thread when the cancel arrives can't be taken back, and isn't counted.

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
