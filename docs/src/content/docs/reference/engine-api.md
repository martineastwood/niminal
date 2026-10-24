---
title: Engine API
description: "JSON-RPC 2.0 over WebSocket for the niminal server: timestamped notes, parameter changes, definitions."
---

**Compiled by niminal.** This is the server's own API: the notes and definitions every client sends, whichever one you use.

The **engine** is the audio server that `niminal server` runs. It holds the instruments, effects, nodes, buses and controls that niminal code defines, and plays timestamped events sent to it: notes, parameter changes, control changes, definitions. It knows nothing of clips, scenes, patterns, tempo, meter or quantize: those belong to a client such as the [live sequencer](/reference/protocol/), which uses this very API. A tracker, a score editor or a bridge to another tool can be a client too.

`niminal server` runs only the generic engine on port **7401** (`--port`). Any client can connect with its own sequencing model. VS Code owns a separate live sequencer process; the server never starts one. Messages are **JSON-RPC 2.0** text frames over **WebSocket**, on loopback only.

## Connection

1. Open a WebSocket to `ws://127.0.0.1:7401/`.
2. Send **`hello`** first, with `{ "protocol": 5, "client": "my-app" }` (and `token` if the server needs one). The result has `sample_rate`, `channels` and the `topics`.
3. Subscribe to topics for push updates: **`clock`** (the [clock](#clock) about ten times a second), **`meters`** (peak levels), **`notices`** (warnings) and **`transactions`** (graph change acknowledgements), **`voices`**, **`reports`**, **`scopes`** and **`jobs`**. The hello result also advertises capabilities and limits.

## Requests

A client's events belong to its connection: when it disconnects, its held notes are released and its notes still waiting never sound. Notes that are already sounding and were sent with a `dur` finish normally.

### `events.send`

One batched method for every timed thing a client can do. The whole batch is checked before any of it is queued.

```json
{ "method": "events.send", "params": { "events": [
  { "op": "note", "target": "lead", "at": 1536000, "dur": "250ms", "args": { "freq": "c4" } },
  { "op": "control", "name": "cutoff", "at": 96000, "value": 2, "unit": "khz", "glide": "50ms" },
  { "op": "routing", "at": "now", "edits": [{ "op": "set", "id": "pad", "args": { "cut": "800hz" } }] }
] } }
```

**`at`** is a nonnegative whole number (an absolute sample) or `"now"` (as soon as possible). `dur` and `glide` are physical durations (`"250ms"`, `"0.5sec"`). Beats and bars must be converted by the client, including durations and parameter values.

| `op` | Fields | Meaning |
| --- | --- | --- |
| `note` | `target`, `at`, optional `to`, `args`, `dur`, `id`, `tag` | Play a note on a node or instrument. With `to`, play the named instrument into that connection’s collection/fx node. `args` are named parameters as in [Score model](/reference/score-model/). With `dur` and no `id` it is fire-and-forget. With an `id` and no `dur` it is held until an `off`. With both it is held for at most `dur`. |
| `set` | `id`, `at`, `args`, optional `glide` | Change parameters of a sounding note, or of one that hasn't started yet (it then starts with the value). Any declared parameter will do. With a `glide`, the instrument’s declared pitch parameter moves evenly in semitones (exponentially in Hz); other parameters move in a straight line. Pitch glides with a nonpositive endpoint use a straight line. |
| `off` | `id`, `at` | Release a note, so its envelope runs its release stage. An `off` before the note starts makes it a short blip. |
| `control` | `name`, `at`, `value`, optional `unit`, `glide` | Move a `ctl`. `unit` defaults to the declaration's unit; compatible units are converted. `glide` defaults to the control's own smoothing. |

An **`id`** is a whole number the client picks, and it belongs to that connection: another client can use the same one. Using an id that is still sounding, or one that was never opened or has been let go of, fails the batch. Because the client picks it, a `note`, a `set` and an `off` can go in one batch with no round trip. When a client disconnects its held notes are released and its notes still waiting never sound.

| `define` | `source`, `at`, optional `dir` | Compile definitions (`instr`, `node`, effects, `bus`, `ctl`, `config`, tables, buffers, `fn`, `const`, samples) and swap them in at `at`, sample-exactly: the new mixer is built ahead and running notes carry over. Performance statements, clips, scenes, arrangements, patterns, input bindings, quantize settings and `tempo`/`meter` are errors. DSP source uses physical time units. `clock.time` is elapsed engine time in seconds; musical clock signals must be supplied by a client through controls. Compile problems come back like `eval`'s. `dir` is where sample files and `use` modules are looked for. |
| `forget` | `at` | Drop every definition, as `new` does. The output layout is kept. |

**Time ownership.** The engine owns a monotonically increasing sample clock. Each client owns its tempo map, meter, musical origin, looping and quantization. Clients convert their positions to samples and send events ahead of playback. A tempo edit can cancel and replace that client's pending events; another client's events stay at their submitted samples. The engine never retimes accepted events.

**Tempo authority.** The engine has no tempo, so it cannot arbitrate one. When several clients play together, exactly one of them must own the tempo map, and the others follow it by converting the same beats to samples with that client's map (for example from a value it publishes on a shared bus). If two clients each keep their own map, their events are all accepted and they drift apart. A client that changes tempo cancels and replaces only its own pending events.

**Events at the same sample.** Events with the same `at` take effect in this order: resets and panics, then definition swaps, `control` changes and buffer writes, then notes, then everything that changes a sounding note (`off`, `set` and parameter mappings). Within one step the order is the order the events were accepted: the order of a batch, and batches in the order they arrived. Two consequences clients can rely on, and that the replay test suite pins:

- An `off` and a `note` that reuse the same `id` at one sample end the old note and start the new one, when the `off` comes first in the batch. A tracker row can release the previous note and play the next with one `id`.
- `control` changes at one sample apply in the order they were sent, so the last one sent is the value that holds.

Notes, controls and the rest in a batch are checked against the definitions in force at their own time, so a batch can `define` an instrument and play it. If anything in a batch is wrong, definitions queued earlier in the same batch are taken back.

**Result:** `{ "accepted": 3, "late": 0, "first": 4800, "horizon": 8192 }`. `late` counts events whose time had already passed: they sounded at once. `first` is the earliest sample any event resolved to, and `horizon` is the sample up to which the engine has prepared audio.

Anything wrong in the batch → **-32001** and nothing is queued; each entry of **`problems`** has an **`event`** field with the index of the event it is about. A batch that doesn't parse → **-32602**.

### Fx instances, buses and connections

Protocol **5** includes a timed `routing` operation inside `events.send`. Each connection owns its routing IDs, fx definitions and instances. Two clients can use the same IDs without collisions. `hello` returns `client_id`; a client can deliberately expose a bus with `shared: true`, and another client can address it using that owner ID. All definitions and buffers sent through `define` are private to the connection, including functions, instruments, samples and controls. Reconnecting starts a fresh namespace. Only buses explicitly marked `shared` cross that boundary.

```json
{ "method": "events.send", "params": { "events": [
  { "op": "routing", "at": 96000, "edits": [
    { "op": "fx", "name": "echo", "source": "(feedback: num = 0.3) { out = in.delay(time: 120ms, feedback: feedback) }" },
    { "op": "node", "id": "channel" },
    { "op": "node", "id": "return", "fx": "echo", "args": { "feedback": 0.5 } },
    { "op": "connect", "id": "send", "from": { "kind": "node", "id": "channel" }, "to": { "kind": "node", "id": "return" }, "gain": 0.25 },
    { "op": "connect", "id": "wet", "from": { "kind": "node", "id": "return" }, "to": { "kind": "master" } }
  ] },
  { "op": "note", "target": "pluck", "to": "channel", "at": 96000, "dur": "250ms", "args": { "freq": "c4" } }
] } }
```

`fx` defines reusable DSP with a parameter signature and body. `in` is incoming audio. A node without an fx instance is a voice collection and audio pass-through. Notes choose their instrument independently with `target`, and their destination with `to`. Node output is silent until connected. Add a second channel-to-master connection for a dry path.

| Routing edit | Fields | Meaning |
| --- | --- | --- |
| `fx` | `name`, `source`, optional `output_channels` | Compile a reusable fx definition. `output_channels` (1–16) selects its native output layout independently of the master; input collections keep the master layout. This supports mono modulation buses and explicit conversion FX. Updating or freeing it leaves existing instances on their original graph; creating/updating a node selects the current definition. |
| `bus` | `id`, `channels`, optional `shared` | Create an audio bus, private by default. Channel count is 1–16 and remains fixed until freed. |
| `node` | `id`, optional `fx`, `instrument`, `args` | Create or update an instance, preserving compatible DSP state. `instrument` is an optional default for notes targeting the node directly. |
| `replace` | `id`, `fx`, optional `args`, `fade` | Replace instance DSP, optionally crossfading; see [live updates](#replacing-an-fx-live). |
| `reset` | `id` | Reset persistent FX DSP state at this event’s sample, such as oscillator phase or delay memory, retaining held voices, parameters and routing. |
| `gain` | `id`, `value`, optional `glide` | Move a connection gain using the lightweight control path. |
| `set` | `id`, `args`, optional `glide` | Change instance parameters. `glide` is seconds, defaults to zero and is at most 60. |
| `connect` | `id`, `from`, `to`, optional `gain`, `glide`, `delayed` | Create/update a connection. Gain defaults to 1; glide is seconds. A delayed connection arrives exactly one engine block later, allowing feedback. |
| `disconnect` | `id` | Remove a connection while retaining both endpoints and their state. |
| `drain` | `id` | Disconnect incoming connections, release held voices and refuse new notes. The fx and outgoing connections remain active so tails can ring. |
| `free_node` | `id` | Immediately remove the node, its voices and attached connections. |
| `free_bus` | `id` | Free an unused bus. A remaining connection/reference makes the batch fail. |
| `free_fx` | `name` | Remove a reusable fx definition; existing instances continue. |
| `clear` | none | Remove this connection’s graph. |

Endpoints are `{ "kind": "node", "id": "…" }`, `{ "kind": "bus", "id": "…", "owner": 7 }` or `{ "kind": "master" }`. Omit a bus’s `owner` for your own bus. The master is an output, not a source. Routing IDs use 1–128 ASCII letters, digits or underscores. Channel widths must match; the compiler’s existing DSP layout conversions remain available when defining processing. Collection-node input uses the engine output layout.

Changes are validated together, including cycle detection and ownership, before anything is queued. Non-delayed feedback is rejected. Connections reuse compiled DSP graphs; topology preparation and allocation happen outside rendering. Existing voices, compatible fx state and unchanged control values survive graph edits. Timing, cancellation, tags and replay work as for other engine events.

To remove an effect gracefully, send `drain` at the cutoff, then `free_node` later after the tail you want. There is no inferred tail deadline. Disconnecting the owning client removes its graph and records the cleanup for replay; other clients’ private graphs remain.

The live client lowers each compiled node into an fx template plus a collection and fx instance, then submits explicit output connections. Two lowering edits are also available: `import` (`name`, `node`) imports a compiled DSP node template and consumes its implicit instance; `bus` can take `source` to use a named DSP bus. `retain` (`nodes`, `fx_decls`, `buses`, `connections`, each an ID list) reconciles the owner’s graph without recreating retained instances. DSP expressions may still contain bus sends; those stay part of their compiled fx graph.

Send routing edits as a `routing` op through `events.send`.

### Live parameters and modulation

These are timed `events.send` operations. Each uses a note `id` owned by the connection and can appear in the same batch as the note that opens that ID:

| `op` | Fields | Meaning |
| --- | --- | --- |
| `array` | `id`, `at`, `param`, `values`, optional `glide`, `tag` | Update every numeric element of a fixed-shape array. `values` is a flat list in row order; count, units and ranges must match. |
| `bind_buffer` | `id`, `at`, `param`, `buffer`, optional `tag` | Bind any currently published, compatible buffer owned by this client, including one allocated after the voice began. |
| `map` | `id`, `at`, `param`, optional `source`, `channel`, `scale`, `offset`, `tag` | Connect a parameter to a named control or bus. `source: null` disconnects and holds the last rendered value. |

```json
{ "method": "events.send", "params": { "events": [
  { "op": "array", "id": 10, "at": "now", "param": "weights", "values": [0.2, 0.7], "glide": "50ms" },
  { "op": "bind_buffer", "id": 10, "at": "now", "param": "source", "buffer": "alternate" },
  { "op": "map", "id": 10, "at": "now", "param": "cutoff", "source": "lfo", "scale": 2000, "offset": 100 }
] } }
```

`channel` is 1-based (default 1). Controls connect directly and follow their smoothing. Bus connections read exactly one engine block behind (64 samples by default, set with the server's `--block-size`), independently of render block splitting, so adding or replacing a connection cannot introduce a routing cycle. `scale` defaults to 1 and `offset` to 0; mapping applies `source * scale + offset` in engine units: Hz, seconds, degrees, semitones or linear gain for dB parameters. Declared parameter ranges clamp the resulting signal. Use `bind_buffer` for resource selection rather than mapping a numeric signal to a resource index.

A scalar `set` does not disconnect an active mapping; explicitly unmap first. A component such as `weights[0]` can be mapped independently. Disconnecting and reconnecting moves prepared storage between the command and voice; rendering never allocates or frees it.

Array and buffer updates use the original voice's parameter schema even after the instrument is redefined. Array updates expand to scalar updates; buffer bindings retain their resource identity in the log. All three operations participate in batch validation, cancellation and disconnect cleanup. `status.instruments` includes `arrays` (element paths) and `buffers` (available resource names), alongside scalar `params`.

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
| `buffers.list` | none | `buffers`: each resource's `name`, `frames`, `channels`, `sample_rate` |
| `buffers.alloc` | `name`, `frames`, optional `channels` (default 1) | Allocate a new named buffer within the client memory budget through a logged definition |
| `buffers.read` | `name`, `offset` (default 0), `count` | `name`, `offset`, `values` (at most 65536 samples) |
| `buffers.load` | `name`, `path`, `channel` (0 for all channels, otherwise 1-based; default 0) | Load WAV audio, resample to the engine rate, allocate if absent, and queue logged writes; an existing buffer must have the resulting frame and channel counts |
| `buffers.save` | `name`, `path` | Save a multichannel 32-bit float WAV snapshot; returns `name`, `path`, `frames` |

Sample updates go in `events.send`:

```json
{ "op": "buffer_write", "name": "take", "at": "now", "offset": 0, "values": [0.25, -0.5], "tag": "edit" }
```

A write contains at most 65536 finite samples within the declared length. It applies before notes at that sample, is validated with the rest of the batch, and supports cancellation and disconnect cleanup. Longer updates use several writes at the same time. `buffers.load` records the decoded values in the session log, so replay does not need the original WAV. Writes apply when rendering reaches their time; reading immediately after queuing a write may still return the previous contents. Snapshots do not stop concurrent recording.

`offset` and `count` measure frames; values are interleaved by frame. Buffers support 1–16 channels. `buffers.read` and `buffer_write` accept at most 65536 samples, including all channels. `buffers.free {name}` removes the published name. Prepared voices keep their references; allocating the same name creates a new generation. `buffers.budget {bytes?}` reads or changes the connection’s memory budget (default 256 MiB), returning `bytes` and `retained_bytes`. Retained generations count until their final reference disappears.

In DSP, `buffer take = zeros(48000, channels: 2)` allocates stereo storage. Read a channel with `take.at(position, channel: 2)`. `take.record(signal, channel: 1, loop: true)` records consecutive samples; multichannel signals write consecutive channels starting at `channel`.

### Reports, scopes and voice lifecycle

Subscribe to `reports`, then use a signal tap in an instrument or fx:

```nml
instr tone(freq: hz) {
  out = (osc(sine, freq) * env[1 | 20ms 0]).report("wave", rate: 30hz)
}
```

`.report("name", rate: 30hz, on: trigger)` passes through its input and emits scalar or array values (up to 64 elements). With `on`, every positive edge is retained, including repeated values. Each tap has a 256-event ring; overflow is counted in `dropped`. `report` notifications carry `name`, `source`, `token` (for notes), absolute `sample`, `values` and cumulative `dropped`. Audio threads only publish to bounded storage. Existing `.osc_out` remains available for external OSC destinations.

`scopes.create {id, endpoint, frames?, rate?}` watches master, a node or an accessible bus. Defaults are 1024 frames at 20 updates/second; limits are 4096 frames, 30 updates/second and 32 scopes per connection. Subscribe to `scopes` for `scope` notifications containing `id`, `sample`, `channels` and interleaved `values`. Remove with `scopes.remove {id}`. Windows are sampled from the current rendered audio and can overlap or skip; use capture for continuous audio.

Every accepted note returns a server `token` in the `events.send` result’s `notes` list, paired with its input `event` index. Subscribe to `voices` for `voice` notifications: `state` (`started`, `released`, `ended`, `dropped`), actual renderer `sample`, `token`, optional client `id`, `target`, `reason` and connection-local `sequence`. Reasons distinguish normal completion, choke, panic, invalid DSP, graph removal and capacity drops. `status.active_voices` supplies the current snapshot and sequence for resynchronization. A notification describes actual playback rather than acceptance into the queue.

### Fx resource controls and file players

Fx signatures accept scalar, range, fixed array and buffer parameters. Routing edits `array {id,param,values,glide?}`, `bind_buffer {id,param,buffer}` and `map {id,param,source?,channel?,scale?,offset?}` update a node at the enclosing routing event’s sample. Mapping sources are `{kind:"control",name:"lfo"}` or `{kind:"bus",id:"mod",owner?:7}`. A null source unmaps and holds the last rendered value. Bus modulation has the same 32-sample delay as note modulation. Bindings accept resources created after the fx, and compatible bindings survive replacement. A replacement that removes or changes the unit of a mapped parameter requires an explicit unmap in the same transaction.

A routing `player` edit creates a disk-backed source node:

```json
{"op":"routing","at":"now","edits":[
  {"op":"player","id":"music","path":"file:1","looping":true},
  {"op":"connect","id":"music_out","from":{"kind":"node","id":"music"},"to":{"kind":"master"}}
]}
```

Player fields: `path`, optional 1-based `channels` selection, `rate` (0.25–4, default 1), `position` in seconds, `playing` (default true), `looping`, `loop_start` and `loop_end` in seconds. `seek {id,seconds}` seeks at the enclosing event time. Node `set` edits control `rate`, `playing`, `loop`, `loop_start` and `loop_end`; modulation supports `rate`. Decoding and seeking run on a worker with a fixed page cache. A realtime cache miss produces silence and increments the player’s `underruns`; offline rendering waits for data. Status exposes player position and failure state. Files must remain unchanged during playback. Reverse and granular disk access are outside this transport player’s contract.

### File transfer, export and live capture

Browser clients upload WAV files with `files.upload {id?,offset?,data?,finish?}`. `data` is base64, at most 256 KiB decoded per chunk; start without `id`, then append using the returned handle and byte offset. Set `finish:true` on the final chunk. Ready handles can replace local paths in buffer loads and player edits. `files.read {id,offset?,count}` returns base64 bytes and `eof`, up to 256 KiB per read. `files.remove {id}` deletes the handle. Handles belong to the connection and temporary files are removed on disconnect.

`render.start` runs an independent offline session with `{source,events,frames,sample_rate?,channels?,limiter?,tail?,resources?,outputs?}`. Defaults: 48000 Hz, stereo, limiter and tail enabled. `source` may be empty when the definitions arrive as events instead (a `transaction` at sample 0 can carry a `dir` for sample paths). Events contain explicit engine sample times and must fall before `frames`; the client supplies its project and schedule. This does not replay the live session’s history. Resources are `{name,path,channel?}` buffer imports. Outputs are `{id,endpoint,path?}`; default output is master. Selected stem endpoints must exist at render start. A missing path produces a downloadable file handle. WAV output is streamed with bounded memory and promotes to RF64 past 4 GiB. Tail rendering stops after one second below −90 dB, capped at 60 seconds.

`capture.start {output,at?}` records an existing live endpoint from an absolute sample or `"now"`; `capture.stop {id,at?}` sets the exact exclusive end sample. Capture includes the actual live performance, with the master after its limiter. A worker drains bounded audio storage; overflow or removal of a source fails visibly instead of silently dropping audio.

Both return a job `id`. Use `jobs.status {id}`, `jobs.cancel {id}` or subscribe to `jobs` for `job` notifications. States are `running`, `completed`, `failed` and `cancelled`; completed results include output file handles. At most two jobs run per connection, and at most four offline renders engine-wide. The most recent 64 terminal jobs per connection are retained. Disconnect cancels outstanding work. Local output paths refer to the server’s filesystem.

### Audio devices

`audio.devices` lists input/output devices and supported formats. `audio.status` reports host state and selected settings. `audio.configure {settings,restart:true}` queues an explicit engine restart; settings include `output`, `input`, `sample_rate`, `channels`, optional `buffer_size` (the device callback size in frames, not the DSP block; see `--block-size`) and `no_audio`. Use device IDs from enumeration or `"default"`. Restart resets the sample clock and disconnects all clients; reconnect and resubmit the project. Unsupported formats fail preflight, and device loss remains visible in host status. An embedded engine without an audio host rejects configuration.

### Per-connection limits

`hello` advertises these in `limits`; a request over one is refused, and nothing in it is applied.

| Limit | Value |
| --- | --- |
| Message size | 4 MiB |
| Buffer write per request | 65,536 samples |
| Buffer size | 256 MiB |
| File transfer chunk | 256 KiB |
| Values in one `report` | 64 |
| Scopes | 8, each up to 4096 frames and 30 updates/second |

The scopes are the biggest steady stream a client can ask for: 32 scopes at the limit are `32 × 4096 × 30` frames per second of every channel of the endpoint, far more than the notification queue below holds, so a client that asks for that gets coalesced updates (`transport_dropped` counts them). Ask for the window and rate you will draw.

### Transport limits

Messages are limited to 4 MiB. Each connection has a bounded notification queue (256 messages / 4 MiB); clock, meter and scope updates coalesce. `transport_dropped` reports coalescing/drops. If reliable notifications cannot fit, the slow connection closes so its client can reconnect and rebuild state. There are no resumable server sessions.

### `events.cancel`

Take back this client's events that are still waiting.

```json
{ "method": "events.cancel", "params": { "tag": "lead", "from": 1536000, "release": true } }
```

Any event in an `events.send` batch can carry a **`tag`**, a string of the client's choosing. `events.cancel` drops the waiting events (notes, `set`s, `off`s, `control` and routing changes) with that tag that are at or after **`from`** (a time as for `at`; default `"now"`). Without `tag` it drops all of this client's waiting events. Other clients' events are never touched. With **`release`: true**, held notes with that tag that are already sounding are also let go of at `from`. A held note whose `off` was taken back is held again.

**Result:** `{ "cancelled": 12, "released": 1 }`

A control change that has already reached the audio thread when the cancel arrives can't be taken back, and isn't counted.


### `panic`

Silence this connection’s voices, players and effect memory. Use `engine.panic` to silence the entire engine. **Result:** `{}`. To do it at a given time, send a `panic` event in `events.send`:

| `op` | Fields | Meaning |
| --- | --- | --- |
| `panic` | `at`, optional `tag` | Silence this connection at `at`. Events waiting for later still play. |

### `status`

The engine's state. **Result** includes:

- **`clock`** — `sample`, `seconds`, `voices`
- **`sample_rate`**, **`channels`**, **`processing_latency`** (DSP samples before the limiter/device)
- **`realtime`** — counters such as `late_events`, `starved_frames`, `capacity_drops`, `deadline_misses`, `max_render_micros`, `mixer_builds` and `planner_panics`
- **`controls`** — each `ctl` and node parameter (named `node::param`): `name`, `value`, `target`, `unit` (db shown in dB)
- **`instruments`** — each instrument's `name` and `params` (`name`, `unit`, `default`, `range`)
- **`nodes`**, **`buses`** — their names
- **`load`** — what each of this connection's nodes costs to render: `name`, `voices` (sounding now), `voice_cpu` and `chain_cpu`, each a share of the time one block has (smoothed over recent blocks; `voice_cpu` sums every render thread's work, so it can pass 1). Collecting it costs the audio thread a clock read per node per block and never allocates or locks.
- **`routing`** — this connection’s fx definitions, buses, instances and connections
- **`clients`** — this connection, when it has something going: `id`, `voices` (the ids of its held notes) and `waiting` (how many of its events are still waiting)

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

## Failures and recovery

The engine carries on after a bug rather than going quiet:

- A panic while handling a request answers that request with `-32603`; the connection and every other client are untouched.
- A panic while planning ahead, or while rendering a block, is caught. The renderer starts clean and planning resumes from the session, so the music may drop out for a moment but does not stop. `status.realtime.planner_panics` counts these, and the server prints a notice.
- A voice or effect chain that produces NaN or infinity is silenced alone (voice end reason `invalid_dsp`); a panic inside one voice ends it with reason `panic`.
- If the process itself dies, the cause is appended to `~/.niminal/crash.log`. The session log written up to that moment is intact: `niminal replay <log> --out set.wav` renders it, and playing it into a new engine restores the same state. Clients reconnect and resubmit their project; there is no automatic restore.

Use `status.load` to find which node is costing the most, and `status.realtime` (`deadline_misses`, `max_render_micros`) to see whether the renderer is keeping up.

## The engine log

The engine records every accepted `events.send` and `events.cancel`, with the times resolved to samples, and every `panic`. Playing the log into a new engine gives the same audio, however the original was divided into blocks; `niminal replay` does that. By default the server writes it under `~/.niminal/sessions/`.

## Minimal client flow

```json
→ {"jsonrpc":"2.0","id":1,"method":"hello","params":{"protocol":5,"client":"example"}}
← {"jsonrpc":"2.0","id":1,"result":{"protocol":5,"sample_rate":48000,"channels":2,"topics":["clock","meters","notices","transactions"]}}

→ {"jsonrpc":"2.0","id":2,"method":"events.send","params":{"events":[
    {"op":"define","at":"now","source":"instr x(freq: hz) { osc(sine, freq) * env[1 | 20ms 0] }\nnode t { instrument = x }"},
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
    { "op": "fx", "name": "echo", "source": "() { out = in.delay(time: 120ms, feedback: 0.3) }" },
    { "op": "node", "id": "return", "fx": "echo" },
    { "op": "connect", "id": "wet", "from": { "kind": "node", "id": "return" }, "to": { "kind": "master" } }
  ]
}
```

`events.send` returns `transactions: [{ "id": 42, "at": 96000 }]` for accepted graph changes (`define`, structural `routing`, `transaction` and `forget`). Control-only operations return no graph receipts. Subscribe to **`transactions`** to receive **`transaction`** notifications for your connection. Each has `id`, scheduled `at`, `status` (`queued`, `applied`, `cancelled` or `dropped`), and `applied_at` (null until applied). Applied acknowledgements come from the DSP renderer, with the actual swap sample; acceptance and the metadata clock alone do not acknowledge application. Notifications are delivered when the server next polls. `status.transactions` contains pending transactions and a bounded recent history. Cancelling uses the existing tag/time API; a dropped transaction was invalidated by an earlier graph change.

The JavaScript and Python clients expose `transaction(...)`. The live sequencer also uses transactions when submitting its definitions and routing.

### Replacing an fx live

`replace` changes the fx of an existing node, retaining its connections and sounding voices:

```json
{ "op": "routing", "at": 96000, "edits": [
  { "op": "fx", "name": "new_echo", "source": "(feedback: num = 0.3) { out = in.delay(time: 180ms, feedback: feedback) }" },
  { "op": "replace", "id": "return", "fx": "new_echo", "fade": 0.02, "args": { "feedback": 0.45 } }
] }
```

Matching parameter names and units retain their current values and smoothing trajectories unless overridden in `args`. New parameters use their defaults; removed parameters disappear. A zero `fade` (the default) preserves compatible DSP memory. A positive fade starts the new DSP fresh and linearly blends the old and new outputs for that duration, in seconds (maximum 60). The old fx receives the same incoming audio during the fade; its remaining tail is discarded at the end. Retired DSP storage is destroyed off the audio thread. To keep a longer independent tail, use separate instances and `drain` instead.

Crossfades require the same output width and fx definitions whose audio inputs come only from `in`, with sends represented by explicit connections. Schedule successive crossfaded replacements after the preceding fade finishes. An unrelated graph edit preserves an ongoing fade. Panic cancels the fade and clears both fx instances' memory; subsequent notes use the replacement immediately.

### Graph inspection and meters

`status.routing` describes nodes and connections using their public IDs. Each node's `params` describes parameter names, units, defaults, smoothing durations, current values and targets. Numeric parameter values use engine units (including linear gain for dB parameters). Connection `gain` and `target` report the current and target linear gain. These values follow control-only edits as well as structural edits.

The **`meters`** topic now also includes `routing`, grouped by client, with `nodes` and `buses` containing `{ "id": "…", "peaks": [0.2, 0.15] }`. Node meters measure output after processing and replacement fades; bus meters measure summed audio. Values are per-channel absolute peaks since the previous meter notification, normally about ten times a second. `status.routing_meters` reads current peaks without resetting them. Meter collection never allocates or locks on the audio thread.
