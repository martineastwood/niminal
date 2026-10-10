---
title: Performance
description: Tracks, clips, scenes, live commands, quantization, and control.
---

The performance layer is session-view style: **clips** carry their instrument and are started and stopped by name, **scenes** start sets of clips, and **tracks** are there when you need a mixer channel with effects. Arrangements sequence the same structures for offline renders.

## Parts: clips with an instrument

A clip with an `instrument:` is a *part*. It is also a track of the same name, which plays it, so you start, mute and change it by its name:

```nml
clip bass {
  instrument: pluck(bright: 0.2)
  notes:  [c2 ~ c2 eb2 ~ g1 bb1 ~]
  gain:   [0db -6db -3db]
}
clip lead {
  instrument: pluck(bright: 0.8)
  notes:  [c4 e4 g4 b4]
}

scene verse { bass, lead }       // short for { bass: bass, lead: lead }

verse.start
bass.reverse
lead.transpose(+5st)
bass.mute
bass.unmute
verse.stop
```

Instruments send to buses from inside their definition (`space += out.gain(-12db)`), and a bus can process what it collects:

```nml
bus space: stereo {
  out = it.reverb(room: 0.8, damp: 0.4).gain(0.8)
}
```

The commands are written after the thing they act on: `start`, `stop`, `mute`, `unmute`, `solo` and `unsolo` on a part, `start` and `stop` on a scene. Anything else after the dot is a change to what the part plays (below). Put other material on a part with `lead.play(answer)`. `hush`, `panic`, `reset` and `unsolo` (every solo at once) act on everything.

## Tracks

A part's track is made for you. Write a `track` yourself when you want more than an instrument: effects on the part, sends, or a channel that only processes a bus. A track has parameters, default instrument, note effects, audio effects, sends, and a mixer. In each stage, **`it`** is that stage’s input.

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

chorus.start @ next 4 bars
```

A bare pattern such as `theme = [c4 e4 g4 b4]` is shorthand for a clip with only a `notes` lane, and everything below works the same on both. A clip with no `length:` runs freely: its cycles keep counting, so `<a b>` and `every` alternate. Give it a `length:` and it loops, starting over each time.

Launching a scene starts the clips it lists and **stops every other track** that has an instrument, so a scene is the whole picture: leave a track out and it falls silent. `~` still works for a track you want to name as silent. To add a fill for a bar, write it as a `play` line instead of a scene.

See [Score model](/reference/score-model/) for arrangement-level tempo, meter, and lanes.

## Live commands

While a scene is playing you can change what a part plays by writing the change after its name. It lands on the next boundary, like a `play`, and the track's clip starts over there:

```nml
verse.start
bass.reverse            // the bass clip now plays backwards
lead.transpose(+5st).thin(25%)
verse.stop              // every part in the scene stops together
```

Each change builds on what the track is playing, in the order you made them, and the tracks panel shows the stack (`▶ riff · reverse → thin(25%)`):

```nml
bass.reverse
bass.thin(25%)          // reversed, then thinned
bass.undo               // takes off the last change you made (here, the thin)
bass.clear              // takes off all of them
```

- One line is one change, so `lead.transpose(+5st).thin(10%)` is undone in one `undo`. A track carries at most 16.
- Redefining the clip itself (`riff = …`) keeps your changes and applies them to the new clip.
- `lead.play(…)`, `lead.start` or starting a scene that has it starts the part fresh, with no changes.
- `thin` drops the same notes for the same amount wherever it is written, so undoing something else doesn't change which notes it drops.
- Only a live session can do this. In a file these lines do nothing, so you can keep them in a scratch file next to your song: write the change into the play line to make it part of the piece.

## Timed commands and the transport

Two small pieces of syntax say *when*, and they answer different questions:

- **`@ next 4 bars`, written after a command, is when the change lands.** It is a musical boundary, counted from now. Use it live: `bass.reverse @ next 4 bars`.
- **`at +4 bars`, written before a command, is when in the timeline it happens,** counted from when the code lands. Use it to lay out a section; the times line up down the left of the file.

```nml
verse.start
at +4 bars chorus.start
at +8 bars bass.mute
at +12 bars hush
```

`at bar 5 chorus.start` is at bar 5 on the session's clock, which keeps counting while the daemon runs, so submitting it again after bar 5 does nothing. Prefer `at +N` in anything you will submit more than once.

`reset` stops everything (like `hush`) and makes the next bar line bar 1, so `at bar 5` and the bar numbers in the status bar count from there again. It lands on the next bar like other commands: send it on its own, then the timeline. In a render, `reset` is a `hush`.

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
