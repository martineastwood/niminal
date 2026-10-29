---
title: Patterns
description: Mini-notation, grids, and pattern transforms.
---

**Read by niminal-live.** Patterns are evaluated by the sequencer while a piece plays; the server only sees the notes that come out of them. Finite event lists from outside niminal use [`.nms` scores](/reference/score-model/).

## Shared symbols

Mini-notation, grids, and envelopes share:

| Symbol | Meaning |
| --- | --- |
| Space | One after another in time |
| Comma | At the same time (chords, parallel clips, channels) |
| `~` | Rest or note off |
| `.` | Hold — notes sustain, values stay |
| `x*2`, `x/2` | Play the step twice as fast, or half as fast |
| `[a [b c]]` | Nested brackets subdivide a step |
| `<a b>` | Alternate one value per cycle |
| `440hz`, `-6db` | Values carry units |

## Mini-notation

Steps divide one cycle evenly (one bar by default). `[c2 g2 bb2 f2]` is four steps per cycle; `.over(2bars)` stretches a cycle.

| Write | Meaning |
| --- | --- |
| `a*2`, `a/2` | Twice as fast, half as fast. A number or a pattern: `a*<2 3>`, `a*[2 4]` |
| `a!3`, `a!`, `a b !` | Repeat a step (`a!` is `a a`; a lone `!` repeats the step before it) |
| `a@3` | Make a step three steps long: `[a@3 b]` gives `a` three quarters |
| `a(3,8)`, `a(3,8,2)` | Spread hits evenly (Euclidean), with an optional rotation. The numbers can be patterns: `a(<3 5>,8)`. A negative count swaps hits and rests |
| `a?`, `a?0.3` | Drop the step at random (half the time, or that chance). Each `?` chooses differently, the same way every time |
| `[a \| b \| c]` | Pick one each cycle, at random |
| `<a b, c d e>` | Several alternations at once |
| `<a@2 b>`, `<a!2 b>` | A step can last, or repeat, over cycles |
| `{a b c}%4` | Polymeter: read the list four steps a cycle, carrying on where it left off (`%` is optional: the first list sets the pace). `{a b, x y z}` squeezes each list to that pace; `%` can be a pattern |
| `~` | A rest, which can take `@` and `!` too |

### Chord names

Add a chord quality after the root and optional octave. An omitted octave defaults to 4:

```nml
notes: [C4maj D3min7 Dmin].over(2bars)
```

This plays C major, D minor seventh rooted in octave 3, then D minor rooted in octave 4.
Spaces sequence chords; commas play them simultaneously. Sharps and flats work too:
`C#4min`, `Bb3maj7`. Root letters may be uppercase or lowercase.

Qualities are `maj` / `major`, `min` / `minor` / `m`, `dim`, `aug`, `sus2`, `sus4`,
`power`, `maj6`, `min6`, `maj7`, `min7`, `dom7`, `dim7`, `half_dim7`, `maj9`, and
`min9`. The full names used by `.chord(...)` also work, as do `m6`, `m7`, and `m9`.
Use `C4dom7` or `Cdom7` for a dominant seventh: `c7` is the single note C in octave 7.
Chord names work with pattern transforms such as `.arpeggiate(up)` and `.strum(25%)`.

## Grids

Each character is one step; spaces group steps for readability. `x` hit, `X` accent, `o` ghost, `.` rest.

```nml
hats = grid[x.x. x.x. x.x. x.xX]

groove = grid {
  kick:  x... x... x... x..x
  snare: .... x... .... x...
  hat:   x.x. x.xX x.x. xxx.
}
```

A multi-row grid selects kit members: each row names a sample, slice (such as `s4`), or an instrument assigned in a named kit, and plays it on its hits. The rows play together. `X` is an accent (+3db) and `o` a ghost note (-12db). Choke groups on the kit (`.choke(g(s4, s12))`) apply to its rows as they do to any pattern.

A row can also:

| Write | Meaning |
| --- | --- |
| `snare(pan: 10deg, gain: -3db): x...` | Set `gain`, `pan`, `begin`, or `pitch` on every hit of the row |
| `hat: [x*2 x x(3,8)]` | Mini-notation in the body (`x`, `X`, `o`, `~`, and the usual `*`, `<>`, Euclid, and so on) |
| `snare: .... x...` when a node is named `snare` | Play those hits on that node (so the snare can have its own effects) |

Muting or stopping the host node does not silence rows routed to other nodes. Each drum node has its own mute and solo. Choke groups still apply only among notes that share a node and kit.

## Lanes for an effect

A clip can drive an effect's parameters, and draw the on/off ones as steps. Give the effect's name a block, one row for each parameter:

```nml
fx break_glitch(stutter: 0..1 = 0, stutter_shrink: 0.5..1 = 1, rewind: 0..1 = 0, crush: 0..1 = 0, crush_bits: 2..16 = 5) { ... }

clip glitched {
  instrument: break_voice
  length: 2bars
  notes: cuts

  break_glitch {
    stutter: .... .... .... xxx. .... .... .... xxx.
    rewind:  .... .... xx.. .... .... .... .... ....
    crush:   .... .... .... .... .... xxxx .... ....

    stutter_shrink: 0.7
    crush_bits:     [8@16 4@16]
  }
}
```

`break_glitch { stutter: x... }` is the lane `break_glitch.stutter: x...` written together with the others, so the row name is the parameter's name and nothing else is assumed about the effect.

| Step | Value |
| --- | --- |
| `.` | 0 |
| `x` | 1 |
| `X` | an accent, +3db (about 1.41) |
| `o` | a ghost, -12db (about 0.25) |

- **Steps:** a row of only `.`, `x`, `X` and `o` is steps, one value for each character, with spaces to group them. Adjacent `x`s are one continuous value, so `xxx.` is a hold three steps long. Steps spread over the clip's `length` like any lane, so rows do not need the same number of steps.
- **Mini-notation among the steps:** the usual operators work on a row of steps, with each `.`, `x`, `X` and `o` still a step of its own: `...<x .>` takes a turn each cycle, `x?` happens half the time (chosen the same way every time), `x*2` plays twice as fast and `x(3,16)` spreads three hits over sixteen steps. The same row can be written in brackets, `[. . . <x .>]`. A step that is off, a rest, or dropped by `?` is an explicit 0, so the lane never keeps the last level after a hit.
- **Anything else is an ordinary lane:** a number, a unit, a mini-notation pattern of values in brackets (`[1 0 <1 0>]`), an envelope. Unlike steps, a value pattern that has a gap in it leaves the lane at its last value. Steps and ordinary lanes mix freely in a block. A row of steps wins over a variable called `x` or `o`, so give a variable that is a whole lane value a longer name.
- **Beside the others:** a step row also works on its own line, `break_glitch.stutter: x...`, anywhere a clip takes lanes.
- **Values stay in range:** a parameter such as `0..1` clamps what it is given, so an accent holds at 1, and `o` on a `0..1` parameter is a quarter of the way on. What a parameter does with the number is up to the effect.
- **When the clip ends:** an effect would otherwise keep the last level a clip gave it (a stutter held on when you stop would repeat forever), so when a clip stops, is stopped by `hush`, or plays its last pass, each parameter it drove goes back to the value the effect declares. A parameter that another running clip also drives is left alone, and so is one in a clip that is changed while it plays.
- **Mistakes:** a row named after a parameter the effect does not have is an error that suggests the closest name, and a lane given both in the block and beside it is an error.

See `examples/glitch.nml` for a complete piece, including the effect it drives.

## Rhythmic audio gates

Inside an `instr` or `fx`, `grid[...]` produces a transport-synced level signal rather than note events. Multiply it by audio, or use `.gate(...)` to shape its opening and closing:

```nml
fx trance_gate {
  out = pad_audio.gate(
    grid[x.x. xx.x .x.. xx.x].over(1bar),
    width: 60%,
    attack: 5ms,
    release: 10ms
  )
}
```

Route the sustained sound into `pad_audio` with a bus send; omit its dry `out` if you want only the gated sound. The sound keeps playing underneath the gate, so chords and sample playback are not retriggered. See `examples/trance_gate.nml` for a complete piece.

- **Timing:** grids repeat against the shared transport, including in an instrument whose note starts later. The default length is one bar; `.over(...)` accepts a positive literal duration in beats or bars. Bar lengths follow the current meter.
- **Levels:** `x` is 1, `.` is 0, `X` is +3db (about 1.41), and `o` is -12db (about 0.25). Spaces only group steps. DSP grids use these characters, rather than mini-notation or multiple rows.
- **Width:** the fraction of each active step occupied by its whole pulse, from `0%` to `100%` (or 0 to 1). Defaults to `100%`; a `.` stays closed at every width.
- **Edges:** attack rises linearly, then the gate holds, then release falls linearly. Both edges fit inside the width. If their sum exceeds the pulse duration, both shrink proportionally and there is no hold.
- **Joined hits:** at `100%` width, adjacent active steps form one pulse, including across the loop boundary. Attack and release apply to that whole run. An entirely active full-width grid stays open, with no opening or closing edges. Accent/ghost levels still change at step boundaries.

`.gate(...)` defaults to a 2ms attack and 8ms release. Its width, attack and release can read live controls; width is clamped to 0..1 and edge times to nonnegative values. Put the grid literal and its `.over(...)`/`.width(...)` chain directly in the gate call.

For a raw level signal with no edge shaping:

```nml
fx chopped {
  gate = grid[x.x. xx.x .x.. xx.x].over(1bar).width(60%)
  out = pad_audio * gate.smooth(3ms)
}
```

`.smooth(3ms)` adds a lag, whose tails can extend beyond the pulse. Use `.gate(...)` when attack and release must fit within its width. A named local gate signal can be shared by multiplying it into several signals, or passed into a DSP function. Functions do not read the transport themselves. Grid notation in clip note patterns keeps its existing hit-triggering behaviour.

## Value patterns vs event patterns

- **Value patterns** — notes, numbers, triggers. Note patterns need a node with an instrument.
- **Event patterns** — name the instrument: `pluck(freq: riff)` or `[kick ~ snare ~]`.
- On event patterns, transforms apply to events. Put effects on a node's `out` (or a bus) so they process the voices.
- **Patterns in arguments** cycle independently: `pluck(freq: [c2 c2 g2 bb2], bright: [0.2 0.8])`.

## Universal controls

Every event accepts:

| Control | Effect |
| --- | --- |
| `pan` | Voice output position |
| `gain` | Level after the instrument |
| `to` | Destination bus |
| `dur` | Note hold time |
| `nudge` | Small timing offset (swing, feel) |

## Pattern operations

Numbers in `fast`, `slow`, `every`, `repeat_each`, `spread_evenly` and `start_later` can be patterns: `fast([<1 2>])`, `spread_evenly(hits: [<3 5>], steps: 8)`. A pattern in an argument is written in brackets. So can the amounts in `transpose`, `drop`, `swing`, `take`, `strum` and the chance in `sometimes`: `transpose([<0st +7st>])`, `drop([<10% 50%>])`, `sometimes([<10% 50%>], fast(2))`.

Some operations take a **change**: `every`, `sometimes`, `often`, `rarely`, `overlay`, `only_between`, `step_through`, `in_cycles`, `cycle_through`, `where` and `stereo_copy`. A change is written as operations on the pattern, with the pattern itself left out: `every(2, reverse)`, `sometimes(25%, transpose(+12st))`, `overlay(fast(2).reverse, after: 1/8beat)`. `same` is the change that does nothing, as in `cycle_through(same, reverse)`.

Arithmetic is an operation too: `sometimes(25%, add(7st))`.

| Operation | What it does |
| --- | --- |
| `fast(2)`, `slow(2)` | Change speed |
| `reverse` | Play each cycle backwards |
| `shift(1/8beat)` | Move earlier or later |
| `every(4, reverse)` | Change every nth cycle |
| `sometimes(25%, fast(2))` | Change a random share of the notes |
| `drop(20%)` | Randomly drop notes |
| `spread_evenly(hits: 5, steps: 8)` | Evenly spaced hits |
| `overlay(transpose(+12st), after: 1/8beat)` | Delayed copy |
| `with_rhythm_of(grid[x..x ..x.])` | Give the notes the rhythm of a grid |
| `repeat_each(2)` | Repeat every event |
| `swing(amount: 20%, every: 1/8)` | Delay every other slot by a share of the slot |
| `only_between(0%, 50%, fast(2))` | Change the part of the cycle between two points |
| `shuffle(4)` | Play the four parts in a new order each cycle, each once |
| `pick_random(4)` | Fill each of four slots with a random part |
| `start_later(4)` | Start a quarter later each cycle |
| `step_through(4, fast(2))` | Change one quarter per cycle, in turn |
| `keep_where(grid[x.x. xxxx])` | Silence where the grid rests |
| `to_steps(16)` | Read a signal 16 times a cycle |
| `loop_start(25%)` | Loop the start of the cycle |
| `where(note < c3, transpose(+12st))` | Change the notes that pass a test on `note` (or `note.freq`) |
| `grid[x.x.].invert` | Swap hits and rests in a grid |
| `back_and_forth` | Forwards one cycle, backwards the next |
| `take(75%)` | Play only the first part of each cycle |
| `crop(25%, 75%)` | Stretch part of each cycle to fill it |
| `squeeze(25%, 75%)` | Squeeze each cycle into part of it, silent elsewhere |
| `echo(3, 1/8beat, fade: -6db)` | Repeat each note, quieter each time |
| `strum(25%)` | Strum chords: each note starts a little after the one before |
| `cycle_through(same, reverse, fast(2))` | A different change each cycle, in turn (`same` is no change) |
| `in_cycles(4, 3, fast(2))` | Change the cycles at or past the third of every four |
| `sometimes(25%, reverse, per: cycle)` | Change a random share of whole cycles instead of notes |
| `often(f)`, `rarely(f)` | `sometimes` at 75% and 25% |
| `if_else(grid[x.x. xxx.], a, b)` | Play `a` where the grid hits and `b` where it rests |
| `take_turns(a, b, c)` | One pattern per cycle, in turn. Give it a `for` to decide each turn from its number (below) |
| `count_to(8)`, `random_int(8)` | The numbers 0 to 7 in order, or a random whole number from 0 to 7 (read it with `to_steps`) |
| `set(bright: 0.9)` | Give a clip lanes in place of its own |

Random operations take an optional `seed`, as in `drop(20%, seed: 2)`; a different seed chooses differently.

## Deciding by cycle number

`take_turns` plays one pattern per cycle, and each pattern gets one of its own cycles per turn. Build the turns with `for` and `if`, using `c.mod(4)` for the remainder, to write a phrase that depends on the cycle number:

```nml
def phrase(groove, fill) {
  take_turns(for c in 0..<16 {
    if c.mod(4) == 3 { fill } else { groove }
  })
}

clip lead { instrument: pluck, notes: phrase([c4 e4 g4 e4], [g4 b4 d5 b4]) }
```

The clip plays the 16 turns, then starts again. For a simple repeating change use `every`, `in_cycles` or `cycle_through` instead.

## Arithmetic

`+`, `-`, `*` and `/` work on patterns. A plain number or interval changes every note (`riff + 7st`, `riff * 2`). Between two patterns, each note of the left one is combined with the value of the right one where it starts, and keeps the left timing:

```nml
riff = [c3 e3 g3 e3]
clip bass_up = riff + [<0 3 5 7>]
```

A plain number added to a note moves it by that many semitones. For other timing use the methods `add`, `sub`, `mul` and `div` with `timing`: `first` (the default), `second` or `both` (only where they overlap): `riff.add([0 3 5 7], timing: second)`.

## Controls that belong to one note

A lane such as `gain: [..]` is read where each note starts. These operations instead set a control on each note itself, so it stays with the note through `reverse`, `shuffle`, `overlay` and the rest. The value can be one thing or a pattern; each note takes the value in force where it starts.

| Operation | What it does |
| --- | --- |
| `gain(-6db)`, `gain([0db -6db])` | How loud each note is |
| `pan(-30deg)`, `pan([-30deg 30deg])` | Where each note sits among the speakers: 0 is straight ahead, positive is right. A note's own pan mixes it down and places it, so it suits mono instruments best |
| `begin(25%)` | Start each sample note a quarter of the way in |
| `stereo_copy(reverse)` | The original on the left and the changed copy on the right (`angle: 45deg` sets how far out; the default is 30deg, a hard pan in stereo) |
| `chop(8)` | Cut each sample note into 8 pieces, each playing the next part of the sample |
| `interleave_chops(4)` | Play the pattern once for each of 4 parts of the sample, in turn |

`pan` and `gain` also work as lanes (`pan: [-30deg 30deg]`). Chopping a note whose instrument isn't a sample reports that it has no `begin` parameter when the note plays.

## Signals

`sine`, `saw`, `tri`, `square`, `rand` and `perlin` are patterns with no events of their own: they have a value at every moment, from 0 to 1. Scale them with `range`, and use them as a clip lane (the value at each note's start) or cut them into steps with `to_steps`:

```nml
clip lead {
  instrument: pluck
  notes: [c3 e3 g3 b3]
  bright: sine.range(0.2, 0.9)
}
cutoffs = saw.range(300hz, 3khz).to_steps(8)
```

## Composition tools

These work on note patterns. All randomness is **seeded per cycle**, so live and offline renders match.

| Operation | What it does |
| --- | --- |
| `in_scale(minor, root: c3)` | Turn scale degrees (`0 1 2`, negatives and values past the octave too) into notes. Scales: `major`, `minor`, `harmonic_minor`, `melodic_minor`, `dorian`, `phrygian`, `lydian`, `mixolydian`, `locrian`, `major_pentatonic`, `minor_pentatonic`, `blues`, `whole_tone`, `chromatic` |
| `chord(minor7)` | Turn each note into a chord: `major`, `minor`, `dim`, `aug`, `sus2`, `sus4`, `power`, `major6`, `minor6`, `major7`, `minor7`, `dom7`, `dim7`, `half_dim7`, `major9`, `minor9` |
| `arpeggiate(up)` | Play a chord one note at a time over its length: `up`, `down`, `updown`, `downup`, `converge`, `diverge` |
| `voice_lead` | Move each chord by octaves to sit as close as possible to the one before |
| `invert_melody(around: c4)` | Mirror the notes around a note |
| `markov` | Play the notes in a new order each cycle, each following what followed it in the original |
| `random_walk(step: 2st)` | Wander from the first note, each up to `step` from the last |
| `range(300hz, 3khz)` | Scale a 0 to 1 pattern or signal to a range |
