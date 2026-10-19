---
title: Engine API
description: "JSON-RPC 2.0 over WebSocket for the sound server inside the daemon: timestamped notes, parameter changes, definitions."
---

The **engine** is a sound server. It holds the instruments, effects, tracks, buses and controls that niminal code defines, and plays timestamped events sent to it: notes, parameter changes, control changes, definitions. It knows nothing of clips, scenes, patterns, tempo, meter or quantize: those belong to a client such as the [live sequencer](/reference/protocol/), which uses this very API. A tracker, a score editor or a bridge to another tool can be a client too.

`niminal daemon` runs only the generic engine on port **7401** (`--port`). Any client can connect with its own sequencing model. VS Code owns a separate live sequencer process; the daemon never starts one. Messages are **JSON-RPC 2.0** text frames over **WebSocket**, on loopback only.

## Connection

1. Open a WebSocket to `ws://127.0.0.1:7401/`.
2. Send **`hello`** first, with `{ "protocol": 3, "client": "my-app" }` (and `token` if the daemon needs one). The result has `sample_rate`, `channels` and the `topics`.
3. Subscribe to topics for push updates: **`clock`** (the [clock](#clock) about ten times a second), **`meters`** (peak levels), **`notices`** (warnings) and **`transactions`** (graph change acknowledgements).

## Requests

A client's events belong to its connection: when it disconnects, its held notes are released and its notes still waiting never sound. Notes that are already sounding and were sent with a `dur` finish normally.

### `events.send`

One batched method for every timed thing a client can do. The whole batch is checked before any of it is queued.

```json
{ "method": "events.send", "params": { "events": [
  { "op": "note", "target": "lead", "at": 1536000, "dur": "250ms", "args": { "freq": "c4" } },
  { "op": "control", "name": "cutoff", "at": 96000, "value": 2, "unit": "khz", "glide": "50ms" },
  { "op": "track", "name": "pad", "at": "now", "args": { "cut": "800hz" } }
] } }
```

**`at`** is a nonnegative whole number (an absolute sample) or `"now"` (as soon as possible). `dur` and `glide` are physical durations (`"250ms"`, `"0.5sec"`). Beats and bars must be converted by the client, including durations and parameter values.

| `op` | Fields | Meaning |
| --- | --- | --- |
| `note` | `target`, `at`, optional `to`, `args`, `dur`, `id`, `tag` | Play a note on a track or instrument. With `to`, play the named instrument into that connection’s collection/processor node. `args` are named parameters as in [Score model](/reference/score-model/). With `dur` and no `id` it is fire-and-forget. With an `id` and no `dur` it is held until an `off`. With both it is held for at most `dur`. |
| `set` | `id`, `at`, `args`, optional `glide` | Change parameters of a sounding note, or of one that hasn't started yet (it then starts with the value). Any declared parameter will do. With a `glide` the parameter moves in a straight line to its new value. |
| `off` | `id`, `at` | Release a note, so its envelope runs its release stage. An `off` before the note starts makes it a short blip. |
| `control` | `name`, `at`, `value`, optional `unit`, `glide` | Move a `ctl`. `unit` defaults to the declaration's unit; compatible units are converted. `glide` defaults to the control's own smoothing. |
| `track` | `name`, `at`, `args`, optional `glide` | Set parameters of a track (what a clip lane like `cut:` sets). `glide` defaults to the parameter's own smoothing. Planned into prepared audio like notes, so a lane can move a parameter many times a second. |

An **`id`** is a whole number the client picks, and it belongs to that connection: another client can use the same one. Using an id that is still sounding, or one that was never opened or has been let go of, fails the batch. Because the client picks it, a `note`, a `set` and an `off` can go in one batch with no round trip. When a client disconnects its held notes are released and its notes still waiting never sound.

| `define` | `source`, `at`, optional `dir` | Compile definitions (`instr`, `track`, effects, `bus`, `ctl`, `config`, tables, buffers, `fn`, `const`, samples) and swap them in at `at`, sample-exactly: the new mixer is built ahead and running notes carry over. Performance statements, clips, scenes, arrangements, patterns, input bindings, quantize settings and `tempo`/`meter` are errors. DSP source uses physical time units. `clock.time` is elapsed engine time in seconds; musical clock signals must be supplied by a client through controls. Compile problems come back like `eval`'s. `dir` is where sample files and `use` modules are looked for. |
| `forget` | `at` | Drop every definition, as `new` does. The output layout is kept. |

**Time ownership.** The engine owns a monotonically increasing sample clock. Each client owns its tempo map, meter, musical origin, looping and quantization. Clients convert their positions to samples and send events ahead of playback. A tempo edit can cancel and replace that client's pending events; another client's events stay at their submitted samples. The engine never retimes accepted events.

Notes, controls and the rest in a batch are checked against the definitions in force at their own time, so a batch can `define` an instrument and play it. If anything in a batch is wrong, definitions queued earlier in the same batch are taken back.

**Result:** `{ "accepted": 3, "late": 0, "first": 4800, "horizon": 8192 }`. `late` counts events whose time had already passed: they sounded at once. `first` is the earliest sample any event resolved to, and `horizon` is the sample up to which the engine has prepared audio.

Anything wrong in the batch → **-32001** and nothing is queued; each entry of **`problems`** has an **`event`** field with the index of the event it is about. A batch that doesn't parse → **-32602**.

### Processor instances, buses and connections

Protocol **3** adds a timed `routing` operation inside `events.send`. Each connection owns its routing IDs, processor definitions and instances. Two clients can use the same IDs without collisions. `hello` returns `client_id`; a client can deliberately expose a bus with `shared: true`, and another client can address it using that owner ID. DSP library definitions sent through `define` remain shared by name.

```json
{ "method": "events.send", "params": { "events": [
  { "op": "routing", "at": 96000, "edits": [
    { "op": "processor", "name": "echo", "source": "(feedback: num = 0.3) { out = it.delay(time: 120ms, feedback: feedback) }" },
    { "op": "node", "id": "channel" },
    { "op": "node", "id": "return", "processor": "echo", "args": { "feedback": 0.5 } },
    { "op": "connect", "id": "send", "from": { "kind": "node", "id": "channel" }, "to": { "kind": "node", "id": "return" }, "gain": 0.25 },
    { "op": "connect", "id": "wet", "from": { "kind": "node", "id": "return" }, "to": { "kind": "master" } }
  ] },
  { "op": "note", "target": "pluck", "to": "channel", "at": 96000, "dur": "250ms", "args": { "freq": "c4" } }
] } }
```

`processor` defines reusable DSP with a parameter signature and body. `it` is incoming audio. A node without a processor is a voice collection and audio pass-through. Notes choose their instrument independently with `target`, and their destination with `to`. Node output is silent until connected. Add a second channel-to-master connection for a dry path.

| Routing edit | Fields | Meaning |
| --- | --- | --- |
| `processor` | `name`, `source` | Compile a reusable processor. Updating or freeing the definition leaves existing instances on their original graph; creating/updating a node selects the current definition. |
| `bus` | `id`, `channels`, optional `shared` | Create an audio bus, private by default. Channel count is 1–16 and remains fixed until freed. |
| `node` | `id`, optional `processor`, `instrument`, `args` | Create or update an instance, preserving compatible DSP state. `instrument` is an optional default for notes targeting the node directly. |
| `replace` | `id`, `processor`, optional `args`, `fade` | Replace instance DSP, optionally crossfading; see [live updates](#replacing-a-processor-live). |
| `gain` | `id`, `value`, optional `glide` | Move a connection gain using the lightweight control path. |
| `set` | `id`, `args`, optional `glide` | Change instance parameters. `glide` is seconds, defaults to zero and is at most 60. |
| `connect` | `id`, `from`, `to`, optional `gain`, `glide`, `delayed` | Create/update a connection. Gain defaults to 1; glide is seconds. A delayed connection arrives exactly one engine block later, allowing feedback. |
| `disconnect` | `id` | Remove a connection while retaining both endpoints and their state. |
| `drain` | `id` | Disconnect incoming connections, release held voices and refuse new notes. The processor and outgoing connections remain active so tails can ring. |
| `free_node` | `id` | Immediately remove the node, its voices and attached connections. |
| `free_bus` | `id` | Free an unused bus. A remaining connection/reference makes the batch fail. |
| `free_processor` | `name` | Remove a reusable definition; existing instances continue. |
| `clear` | none | Remove this connection’s graph. |

Endpoints are `{ "kind": "node", "id": "…" }`, `{ "kind": "bus", "id": "…", "owner": 7 }` or `{ "kind": "master" }`. Omit a bus’s `owner` for your own bus. The master is an output, not a source. Routing IDs use 1–128 ASCII letters, digits or underscores. Channel widths must match; the compiler’s existing DSP layout conversions remain available when defining processing. Collection-node input uses the engine output layout.

Changes are validated together, including cycle detection and ownership, before anything is queued. Non-delayed feedback is rejected. Connections reuse compiled DSP graphs; topology preparation and allocation happen outside rendering. Existing voices, compatible processor state and unchanged control values survive graph edits. Timing, cancellation, tags and replay work as for other engine events.

To remove an effect gracefully, send `drain` at the cutoff, then `free_node` later after the tail you want. There is no inferred tail deadline. Disconnecting the owning client removes its graph and records the cleanup for replay; other clients’ private graphs remain.

The live client lowers each compiled track into a processor template plus a collection and processor instance, then submits explicit output connections. Two lowering edits are also available: `import` (`name`, `track`) imports a compiled DSP track template and consumes its implicit instance; `bus` can take `source` to use a named DSP bus. `retain` (`nodes`, `processors`, `buses`, `connections`, each an ID list) reconciles the owner’s graph without recreating retained instances. DSP expressions may still contain bus sends; those stay part of their compiled processor graph.

The JavaScript and Python clients expose `routing(edits, …)` as a convenience over `events.send`. See [`clients/examples/routing.js`](https://github.com/martineastwood/niminal/blob/main/clients/examples/routing.js) for independently controlled effect returns.

### Live parameters and modulation

These are timed `events.send` operations. Each uses a note `id` owned by the connection and can appear in the same batch as the note that opens that ID:

| `op` | Fields | Meaning |
| --- | --- | --- |
| `array` | `id`, `at`, `param`, `values`, optional `glide`, `tag` | Update every numeric element of a fixed-shape array. `values` is a flat list in row order; count, units and ranges must match. |
| `bind_buffer` | `id`, `at`, `param`, `buffer`, optional `tag` | Select a shared buffer prepared with this voice's original instrument graph. |
| `map` | `id`, `at`, `param`, optional `source`, `channel`, `scale`, `offset`, `tag` | Connect a parameter to a named control or bus. `source: null` disconnects and holds the last rendered value. |

```json
{ "method": "events.send", "params": { "events": [
  { "op": "array", "id": 10, "at": "now", "param": "weights", "values": [0.2, 0.7], "glide": "50ms" },
  { "op": "bind_buffer", "id": 10, "at": "now", "param": "source", "buffer": "alternate" },
  { "op": "map", "id": 10, "at": "now", "param": "cutoff", "source": "lfo", "scale": 2000, "offset": 100 }
] } }
```

`channel` is 1-based (default 1). Controls connect directly and follow their smoothing. Bus connections read exactly 32 samples behind, independently of render block splitting, so adding or replacing a connection cannot introduce a routing cycle. `scale` defaults to 1 and `offset` to 0; mapping applies `source * scale + offset` in engine units: Hz, seconds, degrees, semitones or linear gain for dB parameters. Declared parameter ranges clamp the resulting signal. Use `bind_buffer` for resource selection rather than mapping a numeric signal to a resource index.

A scalar `set` does not disconnect an active mapping; explicitly unmap first. A component such as `weights[0]` can be mapped independently. Disconnecting and reconnecting moves prepared storage between the command and voice; rendering never allocates or frees it.

Array and buffer updates use the original voice's parameter schema even after the instrument is redefined. Array/buffer events are recorded as equivalent named scalar updates for deterministic replay. All three operations participate in batch validation, cancellation and disconnect cleanup. `status.instruments` includes `arrays` (element paths) and `buffers` (available resource names), alongside scalar `params`.

### `clock`

Where the engine is in time, for a client that schedules against it.

**Result**

| Field | Meaning |
| --- | --- |
| `sample` | The sample the output is at, as the audio callback last said. |
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
{ "method": "events.cancel", "params": { "tag": "lead", "from": 1536000, "release": true } }
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

- **`clock`** — `sample`, `seconds`, `voices`
- **`sample_rate`**, **`channels`**, **`processing_latency`** (DSP samples before the limiter/device)
- **`realtime`** — counters such as `late_events`, `starved_frames`, `capacity_drops`, `deadline_misses`, `max_render_micros` and `mixer_builds`
- **`controls`** — each `ctl` and track parameter (named `track::param`): `name`, `value`, `target`, `unit` (db shown in dB)
- **`instruments`** — each instrument's `name` and `params` (`name`, `unit`, `default`, `range`)
- **`tracks`**, **`buses`** — their names
- **`routing`** — each owner’s processor definitions, buses, instances and connections
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
→ {"jsonrpc":"2.0","id":1,"method":"hello","params":{"protocol":3,"client":"example"}}
← {"jsonrpc":"2.0","id":1,"result":{"protocol":3,"sample_rate":48000,"channels":2,"topics":["clock","meters","notices","transactions"]}}

→ {"jsonrpc":"2.0","id":2,"method":"events.send","params":{"events":[
    {"op":"define","at":"now","source":"instr x(freq: hz) { osc(sine, freq) * env[1 | 20ms 0] }\ntrack t { instrument = x }"},
    {"op":"note","target":"t","at":96000,"dur":"250ms","args":{"freq":"c4"}}]}}
← {"jsonrpc":"2.0","id":2,"result":{"accepted":2,"late":0,"first":0,"horizon":8192}}

→ {"jsonrpc":"2.0","id":3,"method":"clock","params":{}}
← {"jsonrpc":"2.0","id":3,"result":{"sample":2048,"sample_rate":48000,...}}
```

## Live graph updates

Use `routing` edits containing only `set` and `gain` for continuous controls. They use timestamped, smoothed control commands without compiling DSP, building mixers or invalidating prepared audio. `gain` takes an existing connection `id`, a finite linear `value`, and optional `glide` in seconds. Use `connect` to change endpoints. When controls accompany structural edits in one routing operation, the whole operation is prepared as a graph change.

```json
{ "op": "routing", "at": 96000, "edits": [
  { "op": "set", "id": "return", "args": { "feedback": 0.4 }, "glide": 0.02 },
  { "op": "gain", "id": "send", "value": 0.5, "glide": 0.02 }
] }
```

### Definition and routing transactions

Use one `transaction` operation for coordinated definition and topology changes at a sample. Its optional `source` and `dir` work like `define`; `edits` work like `routing`. Definitions are compiled first and the final graph is validated and materialized once. Intermediate routing states need not be valid. A rejected transaction leaves the running graph and previously accepted future changes intact. Independent operations in `events.send` retain their own tags and cancellation boundaries; use `transaction` to combine them into one graph change.

```json
{ "op": "transaction", "at": 96000, "tag": "edit-42",
  "source": "instr tone(freq: hz) { osc(sine, freq) * env[0.1 | 100ms 0] }",
  "edits": [
    { "op": "processor", "name": "echo", "source": "() { out = it.delay(time: 120ms, feedback: 0.3) }" },
    { "op": "node", "id": "return", "processor": "echo" },
    { "op": "connect", "id": "wet", "from": { "kind": "node", "id": "return" }, "to": { "kind": "master" } }
  ]
}
```

`events.send` returns `transactions: [{ "id": 42, "at": 96000 }]` for accepted graph changes (`define`, structural `routing`, `transaction` and `forget`). Control-only operations return no graph receipts. Subscribe to **`transactions`** to receive **`transaction`** notifications for your connection. Each has `id`, scheduled `at`, `status` (`queued`, `applied`, `cancelled` or `dropped`), and `applied_at` (null until applied). Applied acknowledgements come from the DSP renderer, with the actual swap sample; acceptance and the metadata clock alone do not acknowledge application. Notifications are delivered when the server next polls. `status.transactions` contains pending transactions and a bounded recent history. Cancelling uses the existing tag/time API; a dropped transaction was invalidated by an earlier graph change.

The JavaScript and Python clients expose `transaction(...)`. The live sequencer also uses transactions when submitting its definitions and routing.

### Replacing a processor live

`replace` changes the processor of an existing node, retaining its connections and sounding voices:

```json
{ "op": "routing", "at": 96000, "edits": [
  { "op": "processor", "name": "new_echo", "source": "(feedback: num = 0.3) { out = it.delay(time: 180ms, feedback: feedback) }" },
  { "op": "replace", "id": "return", "processor": "new_echo", "fade": 0.02, "args": { "feedback": 0.45 } }
] }
```

Matching parameter names and units retain their current values and smoothing trajectories unless overridden in `args`. New parameters use their defaults; removed parameters disappear. A zero `fade` (the default) preserves compatible DSP memory. A positive fade starts the new DSP fresh and linearly blends the old and new outputs for that duration, in seconds (maximum 60). The old processor receives the same incoming audio during the fade; its remaining tail is discarded at the end. Retired DSP storage is destroyed off the audio thread. To keep a longer independent tail, use separate instances and `drain` instead.

Crossfades require the same output width and processors whose audio inputs come only from `it`, with sends represented by explicit connections. Schedule successive crossfaded replacements after the preceding fade finishes. An unrelated graph edit preserves an ongoing fade.

### Graph inspection and meters

`status.routing` describes nodes and connections using their public IDs. Each node's `params` describes parameter names, units, defaults, smoothing durations, current values and targets. Numeric parameter values use engine units (including linear gain for dB parameters). Connection `gain` and `target` report the current and target linear gain. These values follow control-only edits as well as structural edits.

The **`meters`** topic now also includes `routing`, grouped by client, with `nodes` and `buses` containing `{ "id": "…", "peaks": [0.2, 0.15] }`. Node meters measure output after processing and replacement fades; bus meters measure summed audio. Values are per-channel absolute peaks since the previous meter notification, normally about ten times a second. `status.routing_meters` reads current peaks without resetting them. Meter collection never allocates or locks on the audio thread.
