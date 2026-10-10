---
title: Performance
description: Tracks, clips, scenes, live commands, quantization, and control.
---

The performance layer is session-view style: **tracks** persist, **clips** launch on them, **scenes** launch sets of clips. Arrangements sequence the same structures for offline renders.

## Tracks

A track has parameters, default instrument, note effects, audio effects, sends, and a mixer. In each stage, **`it`** is that stage’s input.

```nml
track bass(tone: hz = 2khz) {
  instrument = pluck(bright: 0.4)
  notes      = it.thin(10%)
  out        = it.drive(amount: 0.2).lpf(cutoff: tone)
  space     += out.gain(-14db)
}
```

- Tracks with an instrument play plain note patterns (MIDI-clip style).
- Tracks are created on first use.
- Mixer: `gain`, `pan`, `mute`, `solo`; `with(quantize: ...)` for launch quantization.
- Re-evaluating a track carries effect state without clicks.

## Clips

A clip is a section for one track: **lanes** on one timeline.

```nml
clip riff {
  length: 2 bars
  notes:  [c2 ~ eb2 g1 ~ c2 bb1 ~]
  gain:   [0db -6db -3db -9db]
  bright: [0.2 0.5 0.8]
  tone:   env[400hz (2 bars).exp 3khz]
}
```

- Lane targets: `notes`, instrument parameters, track parameters, universal controls. Unknown lanes are errors. Clip lanes target only their track; scene/arrangement lanes can target any track.
- **Step lanes** read at note start and cycle (polymeter); **`.per_note`** maps nth note → nth value.
- Envelope/signal lanes run on the clip timeline, restart on launch, loop with the clip.
- Shorthand: `[c2 ~ eb2]` → `clip { notes: [...] }`. **`load("...nms")`** is also a clip.
- **`riff.set(bright: [0.9])`** — lanes are fields.

## Scenes and arrangement

```nml
scene verse  { bass: riff, drums: groove, keys: ~ }
scene chorus { bass: riff.transpose(+5st), drums: groove, keys: chords }

launch chorus @ next 4 bars
```

See [Score model](/reference/score-model/) for arrangement-level tempo, meter, and lanes.

## Live commands

```nml
play bass = riff
play bass = riff.fast(2)
mute drums @ next bar
solo bass
unsolo
stop keys
hush
panic
```

**Bindings are reactive:** redefining `riff = ...` updates every track playing it at each track’s quantize point.

## Quantized changes

Code compiles immediately; changes **land** on a musical boundary. Quantum (most specific first): per evaluation (`@ next 4 bars`), per definition (`with(quantize:)`), per kind in `config`, global default.

```nml
config {
  quantize: {
    default: 1 bar
    play:    cycle
    instr:   1 beat
    track:   1 bar
    ctl:     now
    tempo:   1 bar
  }
}

tempo 124bpm @ next bar over 2 bars
```

Boundaries: `now`, `beat`, `bar`, `N beats`, `N bars`, `cycle`. One evaluation is one transaction. Last write wins for pending changes.

## Live coding model

- Evaluate block/line while performing; whole file when rendering.
- Failed compile changes nothing.
- Redefinition: new notes use new defs; sounding notes finish on old ones.
- Tracks and effects keep delay/filter state across edits.

## External control

**`ctl`** — named values from code, MIDI, OSC, or modulators:

```nml
ctl filth  = midi.cc(21)
ctl volume = midi.cc(7).range(-60db..0db)
ctl drive  = 0.3

track bass { out = it.lpf(cutoff: filth.range(200hz..4khz)).drive(amount: filth) }
```

**Keyboards:** `midi.keys` with `note`, `velocity`, `pressure` as signals.

**Triggers:** `key("r")`, MIDI notes, `osc_in`. **Inputs:** `input(1)` or `input(1..2)`.

## Modules

A module is a file. `use rig` loads `rig.nml` (neighbor, project library, packages). Core library is always in scope. One **`config`** per project. Circular `use` is an error.

```nml
use rig
use score/verse
use acme_verbs as av

out = sig.av::shimmer(size: 0.9)
```
