---
title: Performance
description: Clips, scenes, live commands, quantization, and control.
---

**Read by niminal-live.** Clips, scenes and arrangements are the sequencer's; the server only receives the notes and changes they produce.

The performance layer is built around **clips**. A clip is bound to an instrument and holds **lanes** on a timeline: notes that trigger it, and parameters it automates. You start, stop, mute, solo and transform clips, and a **scene** is a list of clips to do that to several at once. An **arrangement** is a timeline of those commands.

## Clips

```nml
clip bass {
  instrument: pluck(bright: 0.2)
  notes:  [c2 ~ c2 eb2 ~ g1 bb1 ~]
  gain:   [0db -6db -3db]
}

bass.start
bass.reverse
bass.mute
bass.unmute
bass.stop
```

Start, stop, mute, solo and transform a clip by its name. The commands are written after the thing they act on: `start`, `stop`, `mute`, `unmute`, `solo` and `unsolo`. Anything else after the dot is a change to what the clip plays (see [Live commands](#live-commands)). `hush`, `panic`, `reset`, `new` and `unsolo` (every solo at once) act on everything. `capture last 32bars as jam` writes the notes that played in that window to `score/jam.nms`.

- **Starting a clip that is already playing does nothing.** It carries on. To begin again, `stop` it and `start` it.
- **A clip has an instrument.** A clip without one can exist, but it plays nothing, and the language server says so. There is no way to play an instrument by naming it: declare a clip that plays it.
- **A clip can be made from another**, keeping its instrument and lanes and applying a change: `clip bass_chorus = bass.transpose(+5st)`. It is a clip of its own, with its own mute and solo.
- **Re-evaluating a definition replaces it by name.** Patterns, clips and scenes share a name space, so changing `clip beat` to `scene beat` replaces the old definition. Use `new` when you want to clear the whole live session.
- **Mute and solo belong to the clip** and stay until you `unmute` or `unsolo` it: `stop`, `start`, `hush`, `panic` and starting a scene do not clear them. `reset` and `new` do. The editor shows them in the status bar (`SOLO bass · 2 muted`) so a clip that has gone quiet is easy to find.
- Voice limits belong to instruments and are counted separately for each clip.

Shared effects live in fx declarations. Buses collect audio from explicit sends:

```nml
bus space: stereo
fx room {
  out = space.reverb(room: 0.8, damp: 0.4).gain(0.8)
}
```

See [Routing](/reference/language/routing/) for shared effects, sends and continuous generators.

### Lanes

```nml
clip riff {
  instrument: pluck
  length: 2bars
  notes:  [c2 ~ eb2 g1 ~ c2 bb1 ~]
  gain:   [0db -6db -3db -9db]
  bright: [0.2 0.5 0.8]
  bass_filter.tone: env[400hz 2bars.exp 3khz]
}
```

Define shared grids and patterns outside the clip, then reference them as `notes: my_grid`. A pattern such as `theme = [c4 e4 g4 b4]` is just notes: it has no instrument, so it is not a clip.

- Envelope lanes, such as `bass_filter.tone: env[300hz 1bar.exp 3khz]`, move an fx parameter over the clip's timeline with the same literal and curves as an instrument envelope; they hold their last level, `.loop` repeats them, and they need no `|`.
- Lane targets: `notes`, instrument parameters, qualified fx parameters (set as each note starts, gliding by the control's smoothing), universal controls. Unknown lanes are errors. Bare lanes target the clip’s notes. Qualified lanes can target an fx shared by several clips. Several lanes of one fx can be written together as a block, `fx_name { open: x.x.  release: 300ms }`, and an on/off lane can be drawn as steps ([lanes for an effect](/reference/language/patterns/#lanes-for-an-effect)).
- **Step lanes** read at note start and cycle (polymeter); **`.per_note`** maps nth note → nth value.
- Envelope/signal lanes run on the clip timeline, restart on launch, loop with the clip. A signal lane is a signal naming an fx parameter, as in `bass_filter.tone: sine.range(300hz, 3khz)`: the parameter follows it continuously (50 readings a second), one cycle per clip length, with or without notes.
- **Length:** `length:` is a number of bars or beats (`2bars`, `6beats`, `3/2bars`; seconds are not accepted). Every lane spans the whole length, and the clip loops at that length, or plays a number of times with `times: 4` and is then finished (not running, and free to be started again). Without a `length:` the clip runs freely, one bar per cycle, and `<a b>` alternates every bar. `riff.over(4bars)` stretches a clip without changing its definition, as a live change you can `undo`.
- **`riff.set(bright: [0.9])`** — lanes are fields.
- **Times and what comes next:** `times: 2` plays the clip's length twice and is then finished; `riff.times(2)` does the same to a running clip as a live change. `then: verse.start` runs a command when the last pass ends, on a clip or a scene: `then: hush`, `then: chorus.start`, `then: bass.stop`. A clip can have several `then:` lines. They are read when the clip is about to finish, so they name clips defined anywhere in the file, and clips can hand over to each other in a loop:

```nml
clip intro { instrument: pluck, notes: [c4 e4], times: 4, then: verse.start }
clip verse { instrument: pluck, notes: [g4 b4], times: 8, then: intro.start }
```

  A clip that is stopped, or replaced by another, before its last pass starts nothing.
- **Longer than a bar:** with `length: 2bars` every lane spans the two bars, so `[c2 ~ eb2 g1]` is four steps over two bars and lanes with the same number of steps line up. A lane `<a b>` is one cycle of the clip, so it plays `a` every time the clip loops; to alternate bar by bar write the bars out (`[<a b>*2]`).

## Scenes

A scene is a list of clips that are started and stopped together:

```nml
scene verse  { bass, drums, lead }
scene chorus { bass_chorus, drums, lead, chords }
scene break  { drums }

verse.start
verse.mute
verse.unmute
verse.stop
```

Scenes play **one at a time**. `chorus.start` switches to `chorus`: the running clips of other scenes that `chorus` does not use stop, `chorus`'s clips start, and it all lands at the same moment, so there is no boundary to miss between a stop and a start. A clip that both scenes use keeps playing, with what had been done to it. A clip belongs to a scene whoever started it, so one you started by hand, such as `lead_outro.start`, stops when a scene that does not use it starts. A clip that is in no scene is never touched by a switch.

```nml
verse.start
chorus.start @ next 4bars   // verse's clips that chorus lacks stop with it, at the same time
chorus.stop                 // stops the scene's clips, and nothing is playing
```

`stop`, `mute`, `unmute`, `solo` and `unsolo` act on every clip of the scene. You can still address a clip directly while a scene has started it: `bass.mute`.

**Changing a scene while it plays.** Evaluating a scene that is playing again changes what is playing to match: clips it gained start and clips it lost stop, all together at the scene's usual time.

```nml
scene verse { bass, lead }
verse.start
scene verse { bass, lead, chords }   // chords starts; the rest carries on
scene verse { lead }                 // bass and chords stop
```

Defining a scene that is not playing only changes the definition. A scene can set when it lands, which then holds for every clip it starts: `scene verse { bass, lead, quantize: 4bars }`.

## Arrangements

An arrangement is a timeline of commands, counted from when it is started. It is launched and stopped like a clip. Its lines are a script, so a scene start in an arrangement only starts: write the stop of the scene it replaces, as below.

```nml
arrangement song {
  at +0bars  verse.start
  at +16bars verse.stop
  at +16bars chorus.start
  at +24bars bass_chorus.mute
  at +26bars bass_chorus.unmute
  at +32bars hush
}

song.start
song.stop        // drops the lines that have not come yet; what is playing carries on
```

Starting an arrangement again while it runs adds another copy, so stop it first. Lines are the same commands as anywhere else, with the times written before them.

`sections` is shorthand for scenes one after another. Each section stops the clips of the one before that it does not use, starts its own, and the arrangement stops everything when the last one ends:

```nml
arrangement song {
  sections: [dawn.over(16bars) build.over(32bars) theme.over(24bars)]
  tempo:    [84bpm 96bpm 96bpm]
  meter:    [4/4 4/4 7/8]
}
```

A section's bars are its own: `over(8bars)` in `7/8` is eight bars of three and a half beats, at that section's tempo, and its clips cycle with that bar. The change is a step at the start of the section. For a tempo that moves gradually, write `tempo: env[84bpm 64bars 84bpm 8bars.log 96bpm]` instead of the list: an envelope over the whole piece, with levels in bpm and times in bars of music. It is played as steps of an eighth of a bar, and a clip playing through a change speeds up with it. Arrangements can also move an fx parameter over the whole piece (`bass_filter.tone: env[800hz 88bars 3khz]`). Per-section tempo and meter, and these envelopes, apply when an arrangement is rendered (`niminal-live render file.nml song`); a live session takes the tempo from `tempo` commands. Notes whose lengths are written in beats (a `dur` lane, an instrument time in beats) still count beats at the file's tempo. `--from` and `--to` positions also count bars at the file's tempo.

See [Score model](/reference/score-model/) for arrangement-level lanes.

## Live commands

While a clip is playing you can change what it plays by writing the change after its name. The change lands on the next loop point of the clip, and the clip **keeps its place in the loop**: a four-bar clip changed in its third bar goes on with the third bar, changed.

```nml
bass.start
bass.reverse            // the bass clip now plays backwards
lead.transpose(+5st).drop(25%)
verse.stop              // every clip in the scene stops together
```

Each change builds on what the clip is playing, in the order you made them, and the clips panel shows the stack (`▶ · reverse → drop(25%)`):

```nml
bass.reverse
bass.drop(25%)          // reversed, then thinned
bass.set(bright: 0.9)   // a lane for every note: a number or a pattern
bass.set(gain: -6db)    // gain lane (replaces the clip's own gain lane)
bass.undo               // takes off the last change you made (here, the gain)
bass.clear              // takes off all of them
```

- `set` gives the clip a lane in place of its own, so typing `bass.set(` offers the lanes the clip's instrument has. Setting the same lane again just replaces the earlier value, and `undo` brings the earlier one back.
- One line is one change, so `lead.transpose(+5st).drop(10%)` is undone in one `undo`. A clip carries at most 16.
- These changes are the one way the clip's source and what plays can differ, and they are always visible in the panel and one `clear` away from gone. To change what a clip plays for good, edit the clip and evaluate it: the clip carries on in place, with your changes on top. Evaluating a pattern the clip uses does the same.
- Stopping a clip and starting it again begins from the clip as written, with no changes.
- `drop` drops the same notes for the same amount wherever it is written, so undoing something else doesn't change which notes it drops.
- A scene is a list of clips and takes no changes: change one of its clips.
- Only a live session can do this. In a file these lines do nothing, so you can keep them in a scratch file next to your song: put the change into the clip to make it part of the piece.

## Timed commands and the transport

Two small pieces of syntax say *when*, and they answer different questions:

- **`@ next 4bars`, written after a command, is when the change lands.** It is a musical boundary, counted from now. Use it live: `bass.reverse @ next 4bars`.
- **`at +4bars`, written before a command, is when in the timeline it happens,** counted from when the code lands. Use it to lay out a section; the times line up down the left of the file. Put them in an [arrangement](#arrangements) to start, stop and cancel the whole timeline by name.

```nml
verse.start
at +4bars verse.stop
at +4bars chorus.start
at +8bars bass.mute
at +12bars hush
```

A command can be written for an absolute time in a file (`at 8bars hush`), counted from the start of the file or from the last `reset`. In a live session the clock keeps counting, so submitting such a line again after its time does nothing. Prefer `at +N` in anything you will submit more than once.

`reset` stops everything (like `hush`), clears every mute and solo, and makes the next bar line bar 1. It lands on the next bar like other commands: send it on its own, then the timeline. In a render, `reset` is a `hush`.

`new` is `reset` for starting another piece in the same live session: it also forgets every definition sent so far (instruments, kits, clips, scenes, tempo), so the next code starts from nothing. Put the new piece's definitions after it in the same evaluation, or send them afterwards. The output layout (`config { channels: ... }`) is kept, because it can only be chosen when the server starts. In a render `new` is a `hush`.

`hush` stops every clip and lets what is sounding ring out. `panic` stops everything at once, effect tails and pending changes included. Neither touches mute or solo.

`capture last 32bars as jam` writes the notes that clips and scored events played in that window to `score/jam.nms`, as a section you can load again. Times are rebased so the window starts at beat 0. In a render or session replay the command does nothing.

## Quantized changes

Code compiles immediately; changes **land** on a musical boundary. Most specific wins:

1. `@ next 4bars` (or similar) on the statement
2. `--quantize` / REPL `:quantize` for that evaluation
3. `quantize` in what the command is about (`quantize: 1beat` in a `scene`, else in a `clip`), or in the definition being changed (`quantize = 1beat` in an `instr`). A scene's setting beats its clips' for that start, so the scene lands as one.
4. `config { quantize: { ... } }` for a whole kind of change
5. Built-in defaults by kind: starting a clip on the next **cycle**, changing or redefining a clip on its next **cycle**, instrument redefinitions on the next **beat**, tempo and most other changes on the next **bar**

```nml
tempo 124bpm @ next bar over 2bars
bass.reverse @ next 4bars
```

Boundaries: `now`, `beat`, `bar`, `N beats`, `N bars`, `cycle`. A **cycle** is the clip's own length: a change to a clip that is playing lands at its next loop point, counted from when it began (one bar for a clip with no `length:`), and anything else waits for the next bar. One evaluation is one transaction. Last write wins for pending changes.

A project sets the quantum of a kind of change in `config`, and one definition can set its own:

```nml
config {
  quantize: {
    default: 1bar     // commands and everything not named below
    play:    cycle     // starting a clip or scene, and changing a clip
    instr:   1beat    // instrument redefinitions
    clip:    cycle    // redefining a clip
    ctl:     now
    tempo:   1bar
  }
}

clip fill { instrument: pluck, notes: [c4 e4 g4 b4], quantize: 1beat }
```

Each quantum is `now`, `beat`, `bar`, `cycle`, `1beat` or `4bars`; a kind left out keeps its built-in one. The settings take effect once the `config` has landed (they are changes like any other), and `config` also takes `channels` (see [Routing](/reference/language/routing/)).

### Tempo and meter

A new tempo keeps every running clip where it is in its bar: a bar that was three eighths played is still three eighths played, only shorter or longer. Commands set for later (`at +8bars …`) keep their place in the music too, and so do the bar lines: if it was bar 4, beat 2.5, it still is.

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

`niminal-live render` plays a file like that through a session, as the server would, so you say how long to render (`--bars 16` or `--seconds 30`). `--stems`, `--arrangement`, `--score`, `--from` and `--to` aren't available for it.

## Score functions

`def` builds clips, scenes and patterns offline (and live, through the same eval path). Unlike DSP `fn`, a `def` never runs on the audio thread: it returns score values.

```nml
def motif(root) {
  stack([root ~ ~ ~], [~ ~ root ~].transpose(+7st))
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

// a def can build a lane for a clip
def fill(root) {
  [root e4 g4]
}

clip fill_clip {
  instrument: pluck
  length: 1bar
  notes: fill(c4)
}
```

Pattern atoms resolve `def` parameters (`[root ~]`), `for` / `if` / arithmetic work on notes and numbers, and `stack` / `cat` / `seq` combine patterns. Top-level `for` and `if` expand notes and commands while compiling:

```nml
for i in 0..<8 {
  at i * 1/2beat pluck(freq: c4 + i * 1st) for 1/4beat
}

if 1 > 0 {
  verse.start
} else {
  chorus.start
}
```

## Live coding model

- Evaluate block/line while performing; whole file when rendering.
- Failed compile changes nothing.
- Redefinition: new notes use new defs; sounding notes finish on old ones. Redefining a `def` updates every clip that uses it on the next eval.
- Instruments and effects keep delay/filter state across edits.

## External control

**`ctl`** — named values from code, MIDI, OSC, or modulators:

```nml
ctl filth  = midi.cc(21)
ctl volume = midi.cc(7).range(-60db..0db)
ctl drive  = 0.3

fx bass { out = in.lpf(cutoff: filth.range(200hz..4khz)).drive(amount: filth) }
```

**Learning a controller:** write `midi.learn` and move the knob you want. The next control change binds to it, and the server tells the editor to rewrite the line to what it heard.

```nml
ctl filth = midi.learn.range(200hz..4khz)      // move a knob...
ctl filth = midi.cc(21).range(200hz..4khz)     // ...and the line becomes this
```

With several `midi.learn` controls waiting, each takes the next control change in the order they appear. Enable MIDI on the live client (`--midi` in a terminal, `niminal.midi` in the editor) to listen to a controller.

**Pads and triggers:** `on` runs commands when a key, a controller or an OSC message arrives, so a pad can launch a scene or a footswitch can stop everything:

```nml
on midi.note(36) beat.start              // pad C1, any channel
on midi.note(37, 10) {                   // pad on channel 10: two commands
  fill.start
  beat.stop
}
on midi.cc(64) bass.reverse              // sustain pedal goes down
on osc_in("/panic") hush                 // a message with a value above 0.5
```

**Gestures:** a clip with rows for an effect and no `instrument:` or notes is a gesture. It plays no sound of its own and only moves the effect's controls, then puts them back when it ends, so a pad can fire a stutter, a roll or a rewind. Give it `times: 1`, a `length:`, a `quantize:` to land it on the beat, and `launch:` to put it on a pad:

```nml
clip roll {
  length: 1bar
  times: 1
  quantize: 1/4beat
  launch: midi.note(37)
  break_stutter {
    hold:   .... .... .... xxxx
    shrink: [1@12 0.7@4]
  }
}
```

The rows step through the clip a sixteenth at a time. A list of levels (`[0@14 3@1 7@1]`) steps a control, and `env[0 3/4bar 0 1/4bar 12]` glides it, so a pitch can climb smoothly through a roll. (Levels in an `env` can't be times, because a time reads as a duration there.) See `examples/gestures.nml`.

**Hold until the key comes up:** a gesture runs for its set length. For a stutter that lasts as long as a pad is held, bind a control to the key: `ctl held = midi.note(39)` is above zero while the key is down and back to zero when it comes up (the velocity sets how far; `.range(...)` sets the span, 0 to 1 if left out). Read it where the effect takes its hold, for example `repeat_last(hold: hold + held, ...)`.

**One knob, many controls:** a `ctl` can feed any number of places, each with a range of its own, so it works as a macro: `ctl open = midi.cc(21)`, then `open.range(200hz..4khz)` on a filter and `open.range(0..0.5)` on a resonance.

**Launching a scene or clip from a pad:** the usual case has a shorter form. Say which key launches it where it is defined, and `launch:` is the same as an `on` line that starts it:

```nml
scene verse  { drums, bass, lead,  launch: midi.note(36) }
scene chorus { drums, bass, pad,   launch: midi.note(37) }
```

Scenes play one at a time, so the pad for the chorus also switches the verse off, and the clips they share carry on. Keys can be given by name (`midi.note(c2)`, which is note 36 in this language's numbering) or number, and a channel as the second argument. `launch:` also takes `midi.cc(64)` and `osc_in("/verse")`.

The commands are the ones you would type, and they land on their usual boundary: `start` waits for the next cycle, so a pad hit a little early still plays in time. A key sets it off as it goes down; a controller or OSC value as it goes down too (64 or more, or above one half) and not again until it has come back up, so holding a pedal runs the command once. `on midi.note(36)` takes keys on every channel, `midi.note(36, 10)` only channel 10. Like controls, triggers listen when MIDI is enabled on the live client (`--midi`, or `niminal.midi` in the editor) or an OSC port is open (`--osc-port`). A command that can't run (a clip that isn't defined yet) is reported as a notice.

**Keyboards:** a clip with `keys:` instead of `notes:` is played from a MIDI keyboard. Start it like any clip, then play:

```nml
clip pad {
  instrument: organ
  keys: midi.keys(channel: 1)      // any channel without the argument
  bright: 0.6                      // other lanes give every note their value
}
pad.start
```

Each key plays the instrument's `freq` from the note number, with the velocity as the note's `gain` (-30db to 0db), and holds it until the key comes up. A clip that is muted, soloed out or stopped doesn't answer the keys. Presses and releases are logged, so a set replays.

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

**OSC on controls:** `ctl x = osc_in("/address")` (`--osc-port` on a terminal live client, `niminal.oscPort` in the editor). See [Live coding](/guide/live-coding/).

## Modules

Split a project across files with `use` — see [Modules](/reference/language/modules/).
