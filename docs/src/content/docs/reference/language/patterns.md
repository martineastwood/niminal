---
title: Patterns
description: Mini-notation, grids, score functions, and pattern transforms.
---

The daemon evaluates **mini-notation** and **grids** itself for endless live patterns, clip lanes, and triggers. Through-composed and row-based writing live in [example clients](/reference/score-model/) (Python, tracker, line, MIDI import).

## Shared symbols

Mini-notation, grids, and envelopes share:

| Symbol | Meaning |
| --- | --- |
| Space | One after another in time |
| Comma | At the same time (chords, parallel parts, channels) |
| `~` | Rest or note off |
| `.` | Hold — notes sustain, values stay |
| `x*2` | Repeat within the step |
| `[a [b c]]` | Nested brackets subdivide a step |
| `<a b>` | Alternate one value per cycle |
| `440hz`, `-6db` | Values carry units |

## Mini-notation

Steps divide one cycle evenly (one bar by default). `[c2 g2 bb2 f2]` is four steps per cycle; `.over(2 bars)` stretches a cycle.

## Grids

Each character is one step; spaces group steps for readability. `x` hit, `X` accent, `o` ghost, `.` rest.

```nml
hats = grid[x.x. x.x. x.x. x.xX]

groove = grid(drums) {
  kick:  x... x... x... x..x
  snare: .... x... .... x...
  hat:   x.x. x.xX x.x. xxx.
}
```

## Score functions

A section can be a function — write a passage once, vary it:

```nml
fn verse(root: note = c3, energy: 0..1 = 0.5) {
  scene {
    lead: clip { notes: [root+12st ~ root+19st ~], gain: [-12db+energy*9db] }
    bass: clip { notes: [root-12st ~ ~ root-5st] }
  }
}

bridge = load("score/bridge.nms")
song = [intro verse(root: c3) verse(root: f3, energy: 0.8) bridge verse(root: c3, energy: 1)]
```

## Value patterns vs event patterns

- **Value patterns** — notes, numbers, triggers. Note patterns need a track with an instrument.
- **Event patterns** — name the instrument: `pluck(freq: riff)` or `[kick ~ snare ~]`.
- On event patterns, transforms apply to events; signal opcodes (`.lpf`) append to each voice’s output.
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

| Tidal | niminal | What it does |
| --- | --- | --- |
| `fast`, `slow` | `fast(2)`, `slow(2)` | Change speed |
| `rev` | `reverse` | Play each cycle backwards |
| `<~`, `~>` | `shift(1/8 beat)` | Move earlier or later |
| `every` | `every(4, it.reverse)` | Change every nth cycle |
| `sometimesBy` | `sometimes(chance: 25%, it.fast(2))` | Random changes |
| `degradeBy` | `thin(20%)` | Randomly drop events |
| `euclid` | `euclid(hits: 5, steps: 8)` | Evenly spaced hits |
| `superimpose`, `off` | `layer(after: 1/8 beat, it.transpose(+12st))` | Delayed copy |
| `jux` | `left_right(it.reverse)` | Original L, changed R |
| `struct` | `rhythm(grid[x..x ..x.])` | Values to a rhythm |
| `ply` | `repeat_each(2)` | Repeat every event |
| `swingBy` | `swing(amount: 20%, every: 1/8 beat)` | Swing |
| `within` | `within(0%..50%, it.fast(2))` | Change part of cycle |
| `shuffle`, `scramble` | `shuffle(4)`, `pick_random(4)` | Rearrange |
| `iter` | `rotate(4)` | Rotate start each cycle |
| `chunk` | `in_turn(4, it.fast(2))` | Change each part in turn |
| `mask` | `mask(grid[x.x. xxxx])` | Silence where mask empty |
| `fix` | `where(it.freq < c3, it.gain(-6db))` | Change matching events |
| `segment` | `to_steps(16)` | Signal → steps |
| `chop`, `striate` | `chop(16)`, `interleave_chops(16)` | Cut sample events |
| `linger` | `repeat_start(25%)` | Loop start of cycle |
| `inv` | `invert` | Swap hits and rests |

## Composition tools

`in_scale`, `chord`, `arpeggiate`, `voice_lead`, `invert_melody`, `retrograde`, `markov`, `random_walk`, `palindrome`, and similar. Randomness is **seeded per cycle** so live and offline renders match.
