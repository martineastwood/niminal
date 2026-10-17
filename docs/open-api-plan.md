# Open daemon API — plan

Status: proposed, 17 October 2026. Work through the phases in order. Each one ends in a working, tested, installed build.

## Goal

Split the daemon into two parts:

- **The engine**: a general sound server. It holds the instruments, effects, tracks, buses and controls defined in niminal, and plays timestamped events: notes, parameter changes, control changes, definition swaps, tempo. It knows nothing about clips, scenes, patterns or quantize.
- **The live sequencer**: today's "text-based Ableton" (clips, scenes, quantize, patterns, live commands, keyboards). It becomes an ordinary **client of the engine's public event API**, with no private access to engine internals.

The VS Code workflow must feel the same as it does now. VS Code keeps talking to the sequencer with today's protocol (`eval`, `launch`, `status`…), and the sequencer drives the engine over the same WebSocket API that any other client would use. If the live approach can be rebuilt on the open API, the API is complete. Other clients (a Renoise-style tracker, a blue-style score editor, later a Tidal or Sonic Pi bridge) then get everything the live sequencer has.

```
VS Code ──(eval, launch, status: today's protocol)──▶ live sequencer ──(events API)──▶ engine
tracker / other clients ────────────────────────────────(events API)────────────────────▶ engine
```

Rules that keep this simple:

1. **One way into the engine.** The sequencer uses only the public event API. In live mode it goes over WebSocket on loopback. In offline render it calls the same request type directly. If the sequencer needs something the API lacks, add it to the API. No side doors.
2. **Pattern evaluation stays in Rust**, in the sequencer. The VS Code extension does not schedule.
3. **The engine owns time.** Tempo, meter and the bar origin live in the engine's transport. That way every client resolves `"8bar"` or `"32beat"` the same way and stays in sync. Clients change the transport with timed events. They never do tempo maths of their own.
4. **One binary.** `niminal daemon` runs both parts: the engine server, plus the sequencer connected to it over loopback. `niminal daemon --engine-only` runs only the engine.
5. **No shims.** This is greenfield. Replace wire methods (`event.stream`, `control.set`) and update the clients in the same change. Old session logs need not replay.

## Non-goals

- A SuperDirt/Tidal bridge. It's a later idea, not part of this plan.
- A plugin system, a node-tree API (scsynth `/s_new` style) or remote, non-loopback access.
- Any change to the language itself.

## What exists today

Read these before starting.

| Piece | Where | Notes |
| --- | --- | --- |
| JSON-RPC over WebSocket | `crates/niminal-cli/src/server.rs`, `crates/niminal-daemon/src/protocol.rs` | `hello`, `eval`, `cancel`, `status`, `subscribe`, the performance API |
| Session (both halves together) | `crates/niminal-daemon/src/session/` | `mod.rs` (pending evals, landing, history, `upcoming`), `eval.rs`, `playing.rs`, `input.rs`, `realtime.rs` |
| `Program` | `crates/niminal-lang/src/program.rs` | Mixes engine definitions (`instruments`, `tracks`, `buses`, `controls`, `tempo`, `master`, `buffers`, `taps`…) with sequencing (`notes`, `performance`, `arrangements`, `quantize`) |
| `event.stream` | `protocol.rs::stream`, `input.rs::stream_events` | Fire-and-forget notes (score-model `Event`). No ids, no cancel, no open-ended notes. |
| `control.set` | `protocol.rs::control`, `input.rs::set_control` | Timed `ctl` change at a sample, with glide |
| Held notes | `input.rs::key_event`, `key_param`, `release_key` | Open-ended notes keyed by an internal `u32`. Per-note `bend`/`pressure`. Only "now". |
| Planner | `realtime.rs::plan_ahead`, `plan_window` | Prepares sample-stamped `Command` windows `lookahead_frames` (8192) ahead. `Kind::Note`, `Release`, `Param`, `Control`, `Swap`, `Reset`. Pending definition changes get their mixer built before they land. |
| Quantize | `crates/niminal-daemon/src/quantize.rs` | Boundary maths, belongs to the sequencer |
| Session log and replay | `crates/niminal-daemon/src/log.rs` (`LogInput`) | Evals, controls, events, keys, expression |
| Input | `crates/niminal-cli/src/input.rs` (MIDI, `OscInput`), `crates/niminal-daemon/src/keyboard.rs`, `controls.rs` | MIDI keys, expression, CC and OSC bindings on `ctl`, `midi.learn` |
| Offline render | `crates/niminal-cli/src/render.rs` (`render`, `render_session`) | Plays a file through a `Session` |
| Clients | `clients/js/index.js`, `clients/python/niminal/__init__.py` | Thin wrappers. VS Code depends on the JS one. |
| Protocol docs | `docs/src/content/docs/reference/protocol.md` | Update in every phase that changes the wire |

**Known problem:** `stream_events`, `key_event`, `key_param` and `release_key` all call `invalidate_live()`. The next `prepare_realtime_step` then throws away prepared windows and plans a `Swap` with a freshly built mixer (`fresh_mixer`). Once the sequencer sends every note as an event, that would happen constantly. Phase 1 fixes it first.

## Engine API (target)

### Concepts

- **Voice id.** An id the client picks, scoped to its connection. The engine maps `(ClientId, id)` to the internal held-note key. Because the client picks it, `note`, `set` and `off` can go in one batch with no round trip.
- **Tag.** An optional string on any event, scoped to the client. Cancellation acts on tags. The sequencer tags by track (or track and clip), a tracker by pattern.
- **Time (`at`).** An integer is an absolute sample. `"now"` means as soon as possible. A unit string (`"32beat"`, `"8.5bar"`, `"12.25sec"`) is an absolute position on the engine's transport, measured from its bar origin. `dur` and `glide` are durations in the usual units.
- **Beat-stamped events follow the tempo.** An event given in beats or bars keeps its musical position if the tempo or meter changes before it plays. The engine stores the musical position and retimes still-waiting events when a transport change lands, which is what `retime` and `rescale_landing` already do for pending changes. Sample- and seconds-stamped events never move.
- **Late events** sound at once, and each reply counts them.

### `events.send`

One batched method that handles every timed thing a client can do:

| op | Fields | Meaning |
| --- | --- | --- |
| `note` | `target`, `at`, `args`, optional `id`, `dur`, `tag` | Start a note on a track or instrument. With `dur` and no `id`: fire-and-forget. With `id` and no `dur`: held until `off`. With both: held for at most `dur`. |
| `set` | `id`, `at`, `args`, optional `glide`, `tag` | Change parameters of a sounding or waiting note: any declared parameter, not only `bend`/`pressure` |
| `off` | `id`, `at`, optional `tag` | Release a note, so its envelope runs its release stage |
| `control` | `name`, `at`, `value`, optional `unit`, `glide`, `tag` | A `ctl` change |
| `track` | `name`, `at`, `args` | A track-level parameter, which is what a clip lane like `gain:` sets today (`Kind::Control`) |
| `define` | `source`, `at` | Compile niminal **definitions** (`instr`, `track`, effects, `bus`, `ctl`, `config`, tables, buffers, `fn`) and swap them in at `at`, using the existing pending-mixer mechanism so the swap is sample-exact. Performance statements in `source` are an error. |
| `transport` | `at`, any of `bpm`, `meter`, `origin: true` | A tempo or meter change, or "this point is bar 1" (`reset`). A ramp is several of these. |
| `forget` | `at` | Drop every definition (what `new` does). The output layout is kept. |

```json
{ "method": "events.send", "params": { "events": [
  { "op": "define", "at": "64beat", "source": "instr pluck(freq: hz) { … }" },
  { "op": "note", "id": 17, "tag": "lead", "at": "64beat", "target": "lead", "args": { "freq": "c4" } },
  { "op": "set",  "id": 17, "at": "65beat", "args": { "cutoff": "800hz" }, "glide": "1/2beat" },
  { "op": "off",  "id": 17, "at": "66beat" },
  { "op": "transport", "at": "68beat", "bpm": 128 }
] } }
```

The whole batch is validated before any of it is queued. On failure: `-32001` with `problems`, each giving the failing event's index. `define` returns its compile problems the same way `eval` does today.

**Reply:** `{ "accepted": n, "late": k, "first": <sample>, "horizon": <prepared_until> }`

### `events.cancel`

`{ "tag": "lead", "from": "64beat", "release": true }` drops this client's waiting events with that tag at or after `from` (default: now). Without `tag`, it drops all of this client's waiting events. `release: true` also sends `off` at `from` to matching held notes that are already sounding. `cancel` also covers waiting `define` and `transport` events, which is how the sequencer's `cancel <id>` will work.

**Reply:** `{ "cancelled": n, "released": m }`

### `panic`, `status`, `clock`, topics

- `panic` stays as it is.
- `status` (engine): transport, defined instruments, tracks, buses and controls with their parameters, realtime metrics, and per client its held voices and queued event count.
- `clock` returns `{ sample, beat, bar, bpm, meter, sample_rate, prepared_until, lookahead, output_latency, wall_micros }`. `wall_micros` is the Unix time in microseconds at which `sample` was rendered. It is recorded in the audio callback next to the clock, not at reply time. The `transport` topic carries the same fields.
- The `meters`, `notices` and `transport` topics stay on the engine.

### Disconnect

When a client disconnects, the engine drops its waiting events and releases its held notes. Already-sounding fire-and-forget notes finish normally.

### Engine log

The engine log records every accepted `events.send` and `events.cancel`, with the resolved samples. That makes replay of live audio exact without re-running the sequencer. `niminal render --replay` replays the engine log.

## Phases

At the end of every phase: run `cargo test --workspace` and `cargo clippy --workspace`, update `protocol.md` and the clients if the wire changed, then rebuild and install the release build (see `AGENTS.md`). Record decisions in this file under **Decisions**.

### Phase 0: baseline

Before changing anything, record what "the same" means.

- Render every example in `examples/` and every fixture the render tests use, and save the outputs (or their hashes) as a baseline under `crates/niminal-cli/tests/baseline/`. Add a test that compares against it.
- Script a live session through the daemon (evals with quantize, `launch … then`, live commands, `undo`, tempo ramp, `cancel`, `hush`, `reset`, held keys) driven by the existing session tests' style, and save its rendered audio as a second baseline.

**Done when:** both baselines exist and pass on `main`. Phases 6–7 must keep them passing. A difference is allowed only if it's understood and recorded under **Decisions**.

### Phase 1: append without replanning

- When an event's resolved start is at or after `prepared_until`, insert it into `upcoming` with **no `invalidate_live()`**. Wake the planner thread without bumping the revision. Check that `sync_realtime`'s `upcoming.retain(...)` keeps notes beyond the prepared region.
- Events inside the prepared region must not rebuild the mixer either. Choose between:
  - **(a)** a `PlanMode` that replans from the event's sample but keeps the live mixer (no `Swap`). This needs a safe way to supersede windows already in the `ready` ring.
  - **(b)** a small ring of single prepared `Command`s that the renderer merges by sample into the window it is playing.

  Choose the simpler one that stays allocation-free on the audio thread.
- Apply the same fast path to `key_event`, `key_param` and `release_key`.
- Add `mixer_builds` to `RenderMetrics`.

**Done when:** 1,000 notes streamed over 10 s in batches ~200 ms ahead cause zero extra mixer builds, zero late events, and audio identical to the offline render. A second run with events inside the prepared region also causes zero extra builds.

### Phase 2: `events.send` with `note`, `control`, `track`

- Add the op enum and batch decoding. Remove `event.stream` and `control.set`.
- Parse absolute transport time for `at`. Beats and bars resolve through the transport, including transport changes that are pending but not yet landed.
- `note` with `dur` maps onto `Upcoming`, `control` onto the `set_control` internals, `track` onto `Kind::Control`.
- Add batch validation and the reply shape. Log to the engine log, and make replay work.
- Update `clients/js`, `clients/python` and any `editors/vscode` callers.

**Done when:** protocol tests cover each op, indexed validation errors, late counting and replay equality.

### Phase 3: voice ids, `set`, `off`

- Add a per-client id table mapping to internal keys. Reusing a live id fails the batch.
- Timed `set` generalises `key_param` to any declared parameter, with glide, and also applies to notes that haven't started yet. `Kind::Param.name` (`&'static str`) becomes a parameter index resolved when planning, with no allocation on the audio thread.
- Timed `off` generalises `release_key`. An `off` before the start keeps the "blip" behaviour.
- Clean up on disconnect.

**Done when:** tests cover held note to set to off, glide against offline render, set and off before the note starts, a duplicate id, disconnect, and replay.

### Phase 4: tags and `events.cancel`

- Store `client` and `tag` on `Upcoming` and on waiting controls. Implement `events.cancel`, including `release`, and cancellation inside the prepared region (phase 1 mechanism).

**Done when:** 4 bars tagged `a`, cancelled from bar 2, plus replacements tagged `b`, render as bar 1 from `a` and bars 2–4 from `b`. The same holds when the cancel lands inside the prepared region.

### Phase 5: transport, `define`, `forget`, `clock`

- Move tempo, meter and bar origin into an engine transport changed by `transport` events. Beat- and bar-stamped waiting events are retimed when a change lands.
- `define`: compile definitions only, build the mixer ahead (reuse the pending/`Swap` machinery), and swap at `at`. Problems come back like `eval`'s. Waiting defines can be cancelled by tag.
- `forget` (for `new`).
- `clock`, with `wall_micros` recorded in `crates/niminal-cli/src/audio.rs`.
- Expand engine `status` so a client can learn each instrument's and track's parameters. The sequencer's LSP-like features (`bass.set(` offering lanes) currently read these from the session.

**Done when:** a test lands a `define` at a beat and the next note uses the new instrument at exactly that sample. A tempo change moves beat-stamped notes but not seconds-stamped ones. `clock` is documented.

### Phase 6: the live sequencer as a client

The main test of the API. Rebuild today's live workflow on top of phases 1–5.

- **New crate `niminal-live`**, carved out of `niminal-daemon/src/session` and `quantize.rs`. It owns: evaluating source (it compiles with `niminal-lang`, keeps clips, scenes, patterns, defs, the play/edit stacks, quantize config), pending changes and quantize landing, follow actions (`then … after`), `at +N`, live commands and `undo`, `capture`, and its own log of evals.
- It drives the engine **only** through an `EngineRequest` / `EngineReply` type: the serde types of the engine API, defined once in `niminal-daemon` and used by both. Live, they go over WebSocket. Offline, the same values are handed straight to an in-process engine. No other access to engine types or state.
- **How it plays:** it runs a scheduler loop against `clock`, with a configurable lookahead (start at ~100 ms). It keeps events sent ahead per track, tagged by track. When a change lands at a boundary, it sends `events.cancel { tag, from: boundary }` plus the new events from the boundary, early enough to beat the lookahead. Definition changes become `define` at the landing sample. Tempo, meter, ramps and `reset` become `transport` events. `new` becomes `forget`.
- **Input moves to the sequencer.** MIDI keys and expression, CC and OSC bindings on `ctl`, and `midi.learn` become sequencer concerns, sent as `note`/`set`/`off`/`control` events at `"now"`. This keeps the engine a pure sound server. The `keys:` clip rules (muted, soloed, stopped parts don't answer) are sequencer state anyway. MIDI and OSC *output* stay in the engine, because they are sample-timed output.
- **The VS Code-facing protocol is unchanged**: same port (7400), same methods, notifications (`landed`, `pending`, `learned`…) and `status` fields. The engine listens on its own port (7401). The extension should not need changes beyond config, if any.
- **Offline render** (`niminal render`, `render_session`, `--score`, `--arrangement`) runs the sequencer against an in-process engine with a virtual clock.
- **Audit first.** Before moving code, list every place the current sequencing code reads engine-side state (sounding notes, control values, `Program` fields, voice counts, the planner) and decide how each one comes through the API, or why it doesn't need to. Put the list under **Decisions**.

**Done when:**
- Both phase 0 baselines pass through the new path.
- All existing daemon and CLI tests pass, moved to the crate they now belong to.
- A live check in VS Code works: evaluate a set, live-edit with quantize, use `undo`, tempo ramps, `cancel`, `hush`, `reset`, `new`, a MIDI keyboard, and `midi.learn`.
- Edits landing at `now` are heard within the sequencer's lookahead plus one engine window. Measure this and record it.
- The engine crate no longer contains clips, scenes, quantize or pattern code.

### Phase 7: clean up the engine

- Delete what phase 6 left unused in the engine: `schedule_for`, `scheduled` upcoming notes, pending evals keyed by quantize, the keyboard and controls input code, `LogInput::Eval`, and so on.
- Re-check the planner's lookahead defaults now that every note arrives as an event.
- Update `docs/src/content/docs/reference/protocol.md` into two references (engine API, live sequencer API) and the architecture notes in `README.md` and `clients/README.md`.

**Done when:** the baselines pass, nothing unused is left (`cargo clippy` with dead-code lints clean), and the docs describe both APIs.

### Phase 8: reference tracker client

A second client, to check that nothing in the API is specific to the sequencer.

- `clients/python/examples/tracker.py` (or a JS example): `define`s an orchestra with instruments, an effect bus and a send. It plays a 4-track pattern of rows (note, instrument, volume, effect column as `set`/`off`) from a scheduler loop against `clock`. Editing a row cancels its tag from the next row and re-sends.
- Run it **at the same time as** a live VS Code session. The two should stay in time through a tempo change made from VS Code.

**Done when:** it plays for a few minutes with no late events, live edits are heard on the next row, stopping it releases its notes, and it stays locked to the live session. Anything awkward gets fixed in the engine, not in the client.

## Decisions

Record each decision with a line of why.

1. Phase 1: (b), a ring of single prepared commands (`Injected`) that the renderer merges by sample. The planner thread builds them from a `fresh` list the session fills, and tags each with the revision so a replan drops them. (a) would have needed a way to supersede windows already in the `ready` ring. Held-key releases of notes that haven't started yet still replan, since the note may be prepared as held. A pending change of definitions or a scheduled panic before the note's start also falls back to a replan.
2. Phase 6 audit: each engine-state read and how it crosses the API.
3. The sequencer's default lookahead, and the measured `now`-edit latency.
4. Is JSON fast enough for dense streams? Measure during phases 6 and 8 before considering a binary transport.
