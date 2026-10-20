---
title: Performance
description: Tracks, clips, scenes, live commands, quantization, and control.
---

The performance layer is built around **clips**, **scenes**, and **tracks**: clips carry their instrument and are started and stopped by name, scenes start sets of clips, and tracks are there when you need a mixer channel with effects. Arrangements sequence the same structures for offline renders.

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

The commands are written after the thing they act on: `start`, `stop`, `mute`, `unmute`, `solo` and `unsolo` on a part, `start` and `stop` on a scene. Anything else after the dot is a change to what the part plays (below). Put other material on a part with `lead.play(answer)`. `hush`, `panic`, `reset`, `new` and `unsolo` (every solo at once) act on everything. `capture last 32bars as jam` writes the notes that played in that window to `score/jam.nms`.

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

- Tracks with an instrument play plain note patterns.
- **Parameters** (`tone: hz = 2khz`) are controls named `bass::tone`, which the track's lines read by their short name. Set one from outside like any `ctl` (a client, or MIDI or OSC bound to it), or change its starting value in the header and re-evaluate: it glides there. A clip lane can't drive one yet, and says so if you try.
- **`notes`** changes the notes of every clip played on the track, with `it` the clip's notes. `notes = it.thin(10%)` drops a tenth of them whatever is playing. A clip played from keys is left as it is. A mistake in it is reported where a clip is played on the track.
- **Short forms:** `track bass = pluck(bright: 0.4)` is a track with only an instrument. Naming an instrument where a part goes, as in `pluck.start` or a scene's `{ pluck: riff }`, makes a track of that name playing it.
- Mixer: `gain = -3db` and `pan = 20deg` lines set the track's fader and placement after `out` (each can be a track parameter or `ctl`; a stereo track is mixed down before it is panned, and sends read `out` before the fader). Mute and solo are the commands `bass.mute` and `bass.solo`.
- Re-evaluating a track carries effect state without clicks.

## Clips

A clip is a section for one track: **lanes** on one timeline.

```nml
clip riff {
  length: 2bars
  notes:  [c2 ~ eb2 g1 ~ c2 bb1 ~]
  gain:   [0db -6db -3db -9db]
  bright: [0.2 0.5 0.8]
  tone:   env[400hz 2bars.exp 3khz]
}
```

- An arrangement can carry envelope lanes on a track's parameter, `bass.tone: env[800hz 88bars 3khz]`, over the whole piece (applied when the arrangement is rendered).
- Envelope lanes, such as `cut: env[300hz 1bar.exp 3khz]`, move a track parameter over the clip's timeline with the same literal and curves as an instrument envelope; they hold their last level, `.loop` repeats them, and they need no `|`.
- Lane targets: `notes`, instrument parameters, track parameters (set as each note starts, gliding by the control's smoothing), universal controls. Unknown lanes are errors. Clip lanes target only their track; scene/arrangement lanes can target any track.
- **Step lanes** read at note start and cycle (polymeter); **`.per_note`** maps nth note → nth value.
- Envelope/signal lanes run on the clip timeline, restart on launch, loop with the clip. A signal lane is a signal named after a track parameter, as in `cut: sine.range(300hz, 3khz)`: the parameter follows it continuously (50 readings a second), one cycle per clip length, with or without notes.
- Shorthand: `[c2 ~ eb2]` → `clip { notes: [...] }`.
- **`riff.set(bright: [0.9])`** — lanes are fields.
- **Longer than a bar:** with `length: 2bars` every lane spans the two bars, so `[c2 ~ eb2 g1]` is four steps over two bars and lanes with the same number of steps line up. A lane `<a b>` is one cycle of the clip, so it plays `a` every time the clip loops; to alternate bar by bar write the bars out (`[<a b>*2]`). Without a `length:` a lane's cycle is one bar. `riff.over(4bars)` stretches the whole clip, and `riff.set(...)` lanes span it too.

## Scenes and arrangement

```nml
scene verse  { bass: riff, drums: groove, keys: ~ }
scene chorus { bass: riff.transpose(+5st), drums: groove, keys: chords }

chorus.start @ next 4bars
```

A bare pattern such as `theme = [c4 e4 g4 b4]` is shorthand for a clip with only a `notes` lane, and everything below works the same on both. A clip with no `length:` runs freely: its cycles keep counting, so `<a b>` and `every` alternate. Give it a `length:` and its lanes span that length and it loops, starting over each time.

Launching a scene starts the clips it lists and **stops every other track** that has an instrument, so a scene is the whole picture: leave a track out and it falls silent. `~` still works for a track you want to name as silent. To add a fill for a bar, write it as a `play` line instead of a scene.

An `arrangement` plays scenes in order, each for a length, and sections can have a tempo and meter of their own, one for each section:

```nml
arrangement song {
  sections: [dawn.over(16bars) build.over(32bars) theme.over(24bars)]
  tempo:    [84bpm 96bpm 96bpm]
  meter:    [4/4 4/4 7/8]
}
```

A section's bars are its own: `over(8bars)` in `7/8` is eight bars of three and a half beats, at that section's tempo, and its clips cycle with that bar. The change is a step at the start of the section. For a tempo that moves gradually, write `tempo: env[84bpm 64bars 84bpm 8bars.log 96bpm]` instead of the list: an envelope over the whole piece, with levels in bpm and times in bars of music. It is played as steps of an eighth of a bar, and a clip playing through a change speeds up with it. Arrangements can also move a track's parameter over the whole piece (`bass.tone: env[800hz 88bars 3khz]`). Notes whose lengths are written in beats (a `dur` lane, an instrument time in beats) still count beats at the file's tempo. `--from` and `--to` positions also count bars at the file's tempo.

See [Score model](/reference/score-model/) for arrangement-level lanes.

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
bass.set(bright: 0.9)   // a lane for every note: a number or a pattern
bass.set(gain: -6db)    // part volume (replaces the clip's own gain lane)
bass.undo               // takes off the last change you made (here, the gain)
bass.clear              // takes off all of them
```

- `set` gives the clip a lane in place of its own, so typing `bass.set(` offers the lanes the part's instrument has. Setting the same lane again just replaces the earlier value, and `undo` brings the earlier one back.
- One line is one change, so `lead.transpose(+5st).thin(10%)` is undone in one `undo`. A track carries at most 16.
- Redefining the clip itself (`riff = …`) keeps your changes and applies them to the new clip.
- `lead.play(…)`, `lead.start` or starting a scene that has it starts the part fresh, with no changes.
- `thin` drops the same notes for the same amount wherever it is written, so undoing something else doesn't change which notes it drops.
- Only a live session can do this. In a file these lines do nothing, so you can keep them in a scratch file next to your song: write the change into the play line to make it part of the piece.

## Timed commands and the transport

Two small pieces of syntax say *when*, and they answer different questions:

- **`@ next 4bars`, written after a command, is when the change lands.** It is a musical boundary, counted from now. Use it live: `bass.reverse @ next 4bars`.
- **`at +4bars`, written before a command, is when in the timeline it happens,** counted from when the code lands. Use it to lay out a section; the times line up down the left of the file.

```nml
verse.start
verse.start then chorus after 4bars   // follow action: chorus 4 bars after verse
at +4bars chorus.start
at +8bars bass.mute
at +12bars hush
```

`then` after a `start` queues another launch relative to the first (or to `at +N` when both are used). Live, the same form works, or the `launch` RPC with a `then` object. Named undo is `bass.undo(reverse)` (also via `edit` with `change: "undo(reverse)"`).

`at bar 5 chorus.start` is at bar 5 on the session's clock, which keeps counting while the daemon runs, so submitting it again after bar 5 does nothing. Prefer `at +N` in anything you will submit more than once.

`reset` stops everything (like `hush`) and makes the next bar line bar 1, so `at bar 5` and the bar numbers in the status bar count from there again. It lands on the next bar like other commands: send it on its own, then the timeline. In a render, `reset` is a `hush`.

`new` is `reset` for starting another piece in the same live session: it also forgets every definition sent so far (instruments, kits, tracks, clips, scenes, tempo), so the next code starts from nothing. Put the new piece's definitions after it in the same evaluation, or send them afterwards. The output layout (`config { channels: ... }`) is kept, because it can only be chosen when the daemon starts. In a render `new` is a `hush`.

`capture last 32bars as jam` writes the notes that clips and scored events played in that window to `score/jam.nms`, as a section you can load again. Times are rebased so the window starts at beat 0. In a render or session replay the command does nothing.

## Quantized changes

Code compiles immediately; changes **land** on a musical boundary. Most specific wins:

1. `@ next 4bars` (or similar) on the statement
2. `--quantize` / REPL `:quantize` for that evaluation
3. `with(quantize: 1beat)` after a definition (`clip`, `scene`, `track`, `instr`)
4. `config { quantize: { ... } }` for a whole kind of change
5. Built-in defaults by kind: plays on the next **cycle**, instrument redefinitions on the next **beat**, tempo and most other changes on the next **bar**

```nml
tempo 124bpm @ next bar over 2bars
bass.reverse @ next 4bars
```

Boundaries: `now`, `beat`, `bar`, `N beats`, `N bars`, `cycle`. One evaluation is one transaction. Last write wins for pending changes.

A project sets the quantum of a kind of change in `config`, and one definition can set its own:

```nml
config {
  quantize: {
    default: 1bar     // commands and everything not named below
    play:    cycle     // starting a clip
    instr:   1beat    // instrument redefinitions
    track:   1bar
    ctl:     now
    tempo:   1bar
  }
}

clip fill { notes: [c4 e4 g4 b4] } with(quantize: 1beat)
```

Each quantum is `now`, `beat`, `bar`, `cycle`, `1beat` or `4bars`; a kind left out keeps its built-in one. The settings take effect once the `config` has landed (they are changes like any other), and `config` also takes `channels` (see [Routing](/reference/language/routing/)).

### Tempo and meter

A new tempo keeps every running clip where it is in its bar: a bar that was three eighths played is still three eighths played, only shorter or longer. Commands set for later (`at +8bars …`, `at bar 9 …`) keep their place in the music too, and so do the bar lines: if it was bar 4, beat 2.5, it still is.

A new `meter` does the same. Going from `4/4` to `3/4` halfway through a bar leaves you halfway through a bar that is now three beats long, and the clips carry on from there with their cycles squeezed into it. Nothing restarts. (A `meter` and a `tempo` that change together keep the bar the same length, so nothing needs to move.)

`over` makes the change gradual:

```nml
tempo 140bpm @ next bar over 8bars     // from where it is now, a quarter of a beat at a time
```

A ramp is a run of ordinary tempo changes, four to a beat, queued at once. They show up in the pending list, and a later `tempo` cancels the ones still waiting. Cancelling the id the ramp gave back (or any one of its steps) cancels the rest of it; `cancel` without an id drops every pending change.

A file can have a ramp too. Keep the starting `tempo` and add the ramp, which sets off from it at the boundary it names:

```nml
tempo 100bpm
tempo 140bpm @ next 4bars over 8bars
```

`nimlive render` plays a file like that through a session, as the daemon would, so you say how long to render (`--bars 16` or `--seconds 30`). `--stems`, `--arrangement`, `--score`, `--from` and `--to` aren't available for it.

## Score functions

`def` builds clips, scenes and patterns offline (and live, through the same eval path). Unlike DSP `fn`, a `def` never runs on the audio thread: it returns score values.

```nml
def motif(root) {
  [root ~ root+7st ~]
}

def stairs(n) {
  for i in 0..<n {
    c4 + i * 1st
  }
}

clip riff {
  instrument: pluck
  notes: cat(motif(c3), stairs(4))
}

// anonymous clips inside a def
def fill() {
  clip {
    length: 1bar
    notes: [c4 e4 g4]
  }
}
lead.play(fill())
```

Pattern atoms resolve `def` parameters (`[root ~]`), `for` / `if` / arithmetic work on notes and numbers, and `stack` / `cat` / `seq` combine patterns. Top-level `for` and `if` expand notes and commands while compiling:

```nml
for i in 0..<8 {
  at i * 1/2beat pluck(freq: c4 + i * 1st) for 1/4beat
}

if 1 > 0 {
  lead.play([c4 e4 g4])
} else {
  lead.play([c3])
}
```

## Live coding model

- Evaluate block/line while performing; whole file when rendering.
- Failed compile changes nothing.
- Redefinition: new notes use new defs; sounding notes finish on old ones. Redefining a `def` updates every clip that uses it on the next eval.
- Tracks and effects keep delay/filter state across edits.

## External control

**`ctl`** — named values from code, MIDI, OSC, or modulators:

```nml
ctl filth  = midi.cc(21)
ctl volume = midi.cc(7).range(-60db..0db)
ctl drive  = 0.3

track bass { out = it.lpf(cutoff: filth.range(200hz..4khz)).drive(amount: filth) }
```

**Learning a controller:** write `midi.learn` and move the knob you want. The next control change binds to it, and the daemon tells the editor to rewrite the line to what it heard.

```nml
ctl filth = midi.learn.range(200hz..4khz)      // move a knob...
ctl filth = midi.cc(21).range(200hz..4khz)     // ...and the line becomes this
```

With several `midi.learn` controls waiting, each takes the next control change in the order they appear. Start the live client with `--midi` to listen to a controller.

**Keyboards:** a clip with `keys:` instead of `notes:` is played from a MIDI keyboard. Start it like any part, then play:

```nml
clip pad {
  instrument: organ
  keys: midi.keys(channel: 1)      // any channel without the argument
  bright: 0.6                      // other lanes give every note their value
}
pad.start
```

Each key plays the instrument's `freq` from the note number, with the velocity as the note's `gain` (-30db to 0db), and holds it until the key comes up. A part that is muted, soloed out or stopped doesn't answer the keys. Presses and releases are logged, so a set replays.

**Expression:** the keyboard's pitch bend, pressure and sustain pedal are read too. An instrument takes them by having parameters with these names, and ignores them otherwise:

```nml
instr organ(freq: hz, bend: st = 0st, pressure: 0..1 = 0) {
  out = osc(saw, freq + bend).lpf(cutoff: 600hz + pressure * 4khz) * env[1 | 10ms 0]
}
```

- **`bend`** (an interval) follows the pitch wheel, two semitones each way. Scale it in the instrument for more (`bend * 6`).
- **`pressure`** (0 to 1) follows channel aftertouch, or one key's polyphonic aftertouch. A held note changes as it comes, and keys played later start at the channel's current value.
- **The sustain pedal** (CC 64) keeps a note sounding after its key comes up, until the pedal does. Pressing the same key again with the pedal down damps the old note.

All three are logged, so a set with them replays exactly.

**MPE:** `keys: midi.keys(mpe: true)` plays from a controller that sends each note on a channel of its own (2 to 16, with 1 as the master channel). Each note takes the pitch bend (48 semitones each way), the pressure and the **`slide`** (CC 74, 0 to 1, a `slide: 0..1` parameter) of its own channel. What the master channel sends (bend of two semitones, pressure, slide, the pedal) reaches every note. `mpe: true` and `channel:` can't be used together.

**OSC on controls:** `ctl x = osc_in("/address")` (start the live client with `--osc-port`). See [Live coding](/guide/live-coding/).

## Modules

Split a project across files with `use` — see [Modules](/reference/language/modules/).