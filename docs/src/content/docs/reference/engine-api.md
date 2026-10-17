---
title: Engine API
description: "JSON-RPC 2.0 over WebSocket for the sound server inside the daemon: timestamped notes, parameter changes, definitions and tempo."
---

The **engine** is a sound server. It holds the instruments, effects, tracks, buses and controls that niminal code defines, and plays timestamped events sent to it: notes, parameter changes, control changes, definitions and tempo. It knows nothing of clips, scenes, patterns or quantize: those belong to a client such as the [live sequencer](/reference/protocol/), which uses this very API. A tracker, a score editor or a bridge to another tool can be a client too.

`niminal daemon` runs only the generic engine on port **7401** (`--port`). Any client can connect with its own sequencing model. VS Code owns a separate live sequencer process; the daemon never starts one. Messages are **JSON-RPC 2.0** text frames over **WebSocket**, on loopback only.

## Connection

1. Open a WebSocket to `ws://127.0.0.1:7401/`.
2. Send **`hello`** first, with `{ "protocol": 1, "client": "my-app" }` (and `token` if the daemon needs one). The result has `sample_rate`, `channels` and the `topics`.
3. Subscribe to topics for push updates: **`transport`** (the [clock](#clock) about ten times a second), **`meters`** (peak levels) and **`notices`** (warnings).

## Requests

A client's events belong to its connection: when it disconnects, its held notes are released and its notes still waiting never sound. Notes that are already sounding and were sent with a `dur` finish normally.

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

### Buffers

| Method | Parameters | Result |
| --- | --- | --- |
| `buffers.list` | none | `buffers`: each resource's `name`, `frames`, `sample_rate` |
| `buffers.alloc` | `name`, `frames` | Allocate a new named buffer (1–4194304 frames) through a logged definition |
| `buffers.read` | `name`, `offset` (default 0), `count` | `name`, `offset`, `values` (at most 65536 samples) |
| `buffers.load` | `name`, `path`, `channel` (1-based, default 1) | Load a WAV channel, resample to the engine rate, allocate if absent, and queue logged writes; an existing buffer must have the resulting length |
| `buffers.save` | `name`, `path` | Save a mono 32-bit float WAV snapshot; returns `name`, `path`, `frames` |

Sample updates go in `events.send`:

```json
{ "op": "buffer_write", "name": "take", "at": "now", "offset": 0, "values": [0.25, -0.5], "tag": "edit" }
```

A write contains at most 65536 finite samples within the declared length. It applies before notes at that sample, is validated with the rest of the batch, and supports cancellation and disconnect cleanup. Longer updates use several writes at the same time. `buffers.load` records the decoded values in the session log, so replay does not need the original WAV. Writes apply when rendering reaches their time; reading immediately after queuing a write may still return the previous contents. Snapshots do not stop concurrent recording.

### Instrument reporting

Instruments and tracks can report a changing signal through `.osc_out("/address", port: 9000)`. Add `on: trigger` to capture a value on each positive edge:

```nml
instr tone(freq: hz) {
  report = (freq / 1hz).osc_out("/note/frequency", port: 9000, on: note.start)
  osc(sine, freq) * env[1 | 20ms 0] + report * 0
}
```

Triggered reports preserve repeated equal values. Each tap has a bounded queue of 256 scalar events shared by its voices; overflow discards the oldest. `rate` (default 30hz, 0.5–200hz) controls how often the sender drains the queue, rather than limiting the trigger rate. Network sending happens outside the audio thread. Offline rendering/replay does not send OSC. Reports currently carry one float and no note ID or timestamp.

### `events.cancel`

Take back this client's events that are still waiting.

```json
{ "method": "events.cancel", "params": { "tag": "lead", "from": "64beat", "release": true } }
```

Any event in an `events.send` batch can carry a **`tag`**, a string of the client's choosing. `events.cancel` drops the waiting events (notes, `set`s, `off`s, `control` and `track` changes) with that tag that are at or after **`from`** (a time as for `at`; default `"now"`). Without `tag` it drops all of this client's waiting events. Other clients' events are never touched. With **`release`: true**, held notes with that tag that are already sounding are also let go of at `from`. A held note whose `off` was taken back is held again.

**Result:** `{ "cancelled": 12, "released": 1 }`

A control change that has already reached the audio thread when the cancel arrives can't be taken back, and isn't counted.


### `panic`

Silence everything at once: every voice and the memory of every effect. **Result:** `{}`. To do it at a given time, send a `panic` event in `events.send`:

| `op` | Fields | Meaning |
| --- | --- | --- |
| `panic` | `at`, optional `tag` | Silence everything at `at`. Events waiting for later still play. |

### `status`

The engine's state. **Result** includes:

- **`transport`** — `sample`, `seconds`, `bar`, `beat`, `bpm`, `voices`
- **`sample_rate`**, **`channels`**
- **`realtime`** — counters such as `late_events`, `starved_frames`, `capacity_drops`, `deadline_misses`, `max_render_micros` and `mixer_builds`
- **`controls`** — each `ctl` and track parameter (named `track::param`): `name`, `value`, `target`, `unit` (db shown in dB)
- **`instruments`** — each instrument's `name` and `params` (`name`, `unit`, `default`, `range`)
- **`tracks`**, **`buses`** — their names
- **`clients`** — each connected client that has something going: `id`, `voices` (the ids of its held notes) and `waiting` (how many of its events are still waiting)

## Error codes

| Code | Meaning |
| --- | --- |
| -32700 | Parse error (invalid JSON) |
| -32600 | Invalid request (missing `jsonrpc`, `method`, …) |
| -32601 | Method not found |
| -32602 | Invalid params |
| -32603 | Internal error (the engine carries on) |
| -32000 | Protocol error (hello, version, token) |
| -32001 | Rejected — a batch of events was refused; see **`data.problems`**, each with the **`event`** index it is about |

**Problem object:** `message`, `help`, `line`, `column`, `span` (`start`, `end`), `in_definition`, `event`.

## The engine log

The engine records every accepted `events.send` and `events.cancel`, with the times resolved to samples, and every `panic`. Playing the log into a new engine gives the same audio, however the original was divided into blocks; `niminal render --replay` does that. By default the daemon writes it under `~/.niminal/sessions/`.

## Minimal client flow

```json
→ {"jsonrpc":"2.0","id":1,"method":"hello","params":{"protocol":1,"client":"example"}}
← {"jsonrpc":"2.0","id":1,"result":{"protocol":1,"sample_rate":48000,"channels":2,"topics":["transport","meters","notices"]}}

→ {"jsonrpc":"2.0","id":2,"method":"events.send","params":{"events":[
    {"op":"define","at":"now","source":"instr x(freq: hz) { osc(sine, freq) * env[1 | 20ms 0] }\ntrack t { instrument = x }"},
    {"op":"note","target":"t","at":"1bar","dur":"1/2beat","args":{"freq":"c4"}}]}}
← {"jsonrpc":"2.0","id":2,"result":{"accepted":2,"late":0,"first":0,"horizon":8192}}

→ {"jsonrpc":"2.0","id":3,"method":"clock","params":{}}
← {"jsonrpc":"2.0","id":3,"result":{"sample":2048,"bar":1,"beat":1.04,"bpm":120.0,...}}
```
