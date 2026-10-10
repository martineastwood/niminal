# niminal for VS Code

Highlighting for `.nml` files, and live evaluation against a running daemon.

## Try it

From the repo root:

```
cd editors/vscode && npm install && node install.js   # then reload VS Code (--cursor for Cursor)
niminal daemon          # or "niminal: Start Daemon in a Terminal"
```

Open `examples/groove.nml`, then:

| Key (mac / other) | Does |
| --- | --- |
| Cmd+Enter / Ctrl+Enter | Evaluate the selection, or the block (lines up to the nearest blank line, widened to balance brackets) |
| Cmd+Shift+Enter / Ctrl+Shift+Enter | Evaluate the whole file |
| Cmd+. / Ctrl+. | Hush |

The **niminal** side bar lists your scenes (click one, or press Cmd+1 to Cmd+9 for the Nth, to launch it) and your tracks, with what each is playing and buttons to mute, solo and stop. Cmd+1..9 replace VS Code's group-switching keys while a niminal file has focus.

Compile errors appear as squiggles at the right place, the status bar shows the bar, beat, tempo, voices and changes waiting to land, and `niminal: Panic` is in the command palette.

Settings: `niminal.port`, `niminal.token`, `niminal.quantize` (for example `next 4 bars`), `niminal.path`.

## Tests

`npm test` runs the block finder and the protocol client against a real daemon (build it first with `cargo build`).

## Live edits and audio

Pending writes to the same instrument, track, pattern, or playback control use
last-write-wins. Replacing a pending write preserves other writes in its
transaction. Independent changes retain their own musical boundaries; changes
that need pending definitions wait for those definitions. Explicit `at ...`
score commands and individual notes are kept in sequence. Failed evaluations
leave the pending queue unchanged.

Track edits retain compatible filter, delay, reverb, oscillator, envelope, and
custom-opcode memory while applying the new wiring and parameter values.
Incompatible state starts fresh: for example, resizing a delay buffer or changing
a custom opcode's compiled body. Sounding notes retain their original instrument
and their sends are remapped by bus name when buses change.

The audio device callback reads a bounded, lock-free queue. The DSP renderer
owns the running mixer and limiter and takes no control locks. A separate planner
constructs voices and graph changes up to 8,192 frames ahead (about 171ms at
48kHz), in 512-frame windows. This event lookahead is independent of the audio
queue: it prepares work without rendering that audio early. Edits invalidate
unplayed windows and replan from the render clock.

Rendering starts ready voices at their exact sample positions, and sends finished
voices and replaced graphs back through a bounded retirement queue for destruction
on the planner thread. Voice storage is reserved before rendering, with 512 normal
voices and another 512 slots for completion and stealing fades. Notes exceeding
reserved capacity are dropped and reported. Newly constructed notes per window and deferred nudged notes are each capped
at four times the configured voice capacity. Rendering continues existing DSP
when preparation falls behind; missed preparation and late changes are reported.

The audio queue normally holds 1,024 frames (about 21ms at 48kHz), in addition to
device and limiter latency; it grows when the device requires larger callbacks.
Transport and landing notifications describe the render clock, which leads
audible output by this audio queue. Late audio is discarded after underruns.
An immediate panic invalidates queued audio and silences running DSP within an
engine block; the planner supplies a clean mixer and the renderer clears limiter
memory.

The daemon's `status` response includes `realtime` counters for late events,
preparation starvation, voice capacity drops, retirement pressure, DSP deadline
misses, and maximum render time in microseconds. These measure the current
session; they do not certify deadlines on every device. Integration tests check
sample timing and zero heap allocations **and deallocations** while rendering
notes, completing voices, swapping compatible effects, and handling panic.

The release stress test covers 12 tracks with sample playback, 128 held voices,
filters, reverbs, live control automation, and repeated quantized edits. On the
development machine the maximum 256-frame render took 0.467ms against a 5.33ms budget at 48kHz, with zero
late events, starvation, capacity drops, or deadline misses. Physical-device
latency and underrun testing remain separate from this synthetic check.

### Live controls

Named controls are typed signals available in instruments and track effects:

```niminal
ctl cutoff = 1khz.smooth(10ms)
ctl volume = -18db

instr tone(freq: hz) { osc(saw, freq).lpf(cutoff: cutoff) }
track lead { instrument = tone
  out = it.gain(volume)
}
```

Controls use a 5ms linear glide by default. Set `.smooth(0ms)` on the declaration
to disable it. Re-evaluating a control glides back to its code value; unrelated
graph edits preserve its current target and glide. Newly started notes join the
same control trajectory as existing notes.

The WebSocket JSON-RPC method `control.set` updates a control without compiling
or replacing any graphs or prepared notes:

```json
{
  "jsonrpc": "2.0",
  "id": 7,
  "method": "control.set",
  "params": {
    "name": "cutoff",
    "value": 2.4,
    "unit": "khz",
    "smooth_ms": 10
  }
}
```

`name` and numeric `value` are required. `unit` defaults to the declaration's
canonical unit: plain numbers, hz, db, sec, deg, or st. Compatible units such as
khz, ms, beats, and bars are converted and checked. `smooth_ms` overrides the
declaration's smoothing for that update (0–60,000ms). Optional `at` is an absolute
sample timestamp; omit it to use the current render clock. The response contains
the requested `at`. Past timestamps, unknown controls, mismatched units, invalid
values, and full update queues are rejected.

`status.controls` reports each control's name, current value, target, and unit.
Values are shown in canonical units; db controls are shown in decibels.
Control changes use their own bounded queue and land inside an engine block.
If a late update lands after its requested sample, its actual sample is
acknowledged into the evaluation log so offline replay reproduces the live audio.
The log file is rewritten when this timing is corrected. `realtime.control_drops`
reports queued updates invalidated by a later change to a control's unit.
MIDI and OSC can feed this same path in a subsequent milestone.

See [live_controls.nml](../../examples/live_controls.nml) for a complete patch.
