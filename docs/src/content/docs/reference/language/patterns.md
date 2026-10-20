---
title: Patterns
description: Mini-notation, grids, and pattern transforms.
---

The server evaluates **mini-notation** and **grids** itself for endless live patterns and clip lanes. Finite event lists from outside niminal use [`.nms` scores](/reference/score-model/).

## Shared symbols

Mini-notation, grids, and envelopes share:

| Symbol | Meaning |
| --- | --- |
| Space | One after another in time |
| Comma | At the same time (chords, parallel parts, channels) |
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

A multi-row grid is a pattern of samples: each row names a sample in a kit (a slice such as `s4` works too) and plays it on its hits, and the rows play together. `X` is an accent (+3db) and `o` a ghost note (-12db). Choke groups on the kit (`.choke(g(s4, s12))`) apply to its rows as they do to any pattern.

A row can also:

| Write | Meaning |
| --- | --- |
| `snare(pan: 10deg, gain: -3db): x...` | Set `gain`, `pan`, `begin`, or `pitch` on every hit of the row |
| `hat: [x*2 x x(3,8)]` | Mini-notation in the body (`x`, `X`, `o`, `~`, and the usual `*`, `<>`, Euclid, and so on) |
| `snare: .... x...` when a track is named `snare` | Play those hits on that track (so the snare can have its own effects) |

Muting or stopping the host track does not silence rows routed to other tracks. Each drum track has its own mute and solo. Choke groups still apply only among notes that share a track and kit.

## Value patterns vs event patterns

- **Value patterns** — notes, numbers, triggers. Note patterns need a track with an instrument.
- **Event patterns** — name the instrument: `pluck(freq: riff)` or `[kick ~ snare ~]`.
- On event patterns, transforms apply to events. Put effects on a track's `out` (or a bus) so they process the voices.
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

Numbers in `fast`, `slow`, `every`, `repeat_each`, `euclid` and `rotate` can be patterns: `fast([<1 2>])`, `euclid(hits: [<3 5>], steps: 8)`. A pattern in an argument is written in brackets.

| Operation | What it does |
| --- | --- |
| `fast(2)`, `slow(2)` | Change speed |
| `reverse` | Play each cycle backwards |
| `shift(1/8beat)` | Move earlier or later |
| `every(4, it.reverse)` | Change every nth cycle |
| `sometimes(25%, it.fast(2))` | Change a random share of the notes |
| `thin(20%)` | Randomly drop events |
| `euclid(hits: 5, steps: 8)` | Evenly spaced hits |
| `layer(after: 1/8beat, it.transpose(+12st))` | Delayed copy |
| `rhythm(grid[x..x ..x.])` | Values to a rhythm |
| `repeat_each(2)` | Repeat every event |
| `swing(amount: 20%, every: 1/8)` | Delay every other slot by a share of the slot |
| `within(0%, 50%, it.fast(2))` | Change the part of the cycle between two points |
| `shuffle(4)` | Play the four parts in a new order each cycle, each once |
| `pick_random(4)` | Fill each of four slots with a random part |
| `rotate(4)` | Start a quarter later each cycle |
| `in_turn(4, it.fast(2))` | Change one quarter per cycle, in turn |
| `mask(grid[x.x. xxxx])` | Silence where mask empty |
| `to_steps(16)` | Read a signal 16 times a cycle |
| `repeat_start(25%)` | Loop the start of the cycle |
| `where(it < c3, it.transpose(+12st))` | Change the notes that pass a test on `it` (or `it.freq`) |
| `grid[x.x.].invert` | Swap hits and rests in a grid |
| `palindrome` | Forwards one cycle, backwards the next |
| `set(bright: 0.9)` | Give a clip lanes in place of its own |

Random operations take an optional `seed`, as in `thin(20%, seed: 2)`; a different seed chooses differently.

## Controls that belong to one note

A lane such as `gain: [..]` is read where each note starts. These operations instead set a control on each note itself, so it stays with the note through `reverse`, `shuffle`, `layer` and the rest. The value can be one thing or a pattern; each note takes the value in force where it starts.

| Operation | What it does |
| --- | --- |
| `gain(-6db)`, `gain([0db -6db])` | How loud each note is |
| `pan(-30deg)`, `pan([-30deg 30deg])` | Where each note sits among the speakers: 0 is straight ahead, positive is right. A note's own pan mixes it down and places it, so it suits mono instruments best |
| `begin(25%)` | Start each sample note a quarter of the way in |
| `left_right(it.reverse)` | The original on the left and the changed copy on the right (`angle: 45deg` sets how far out; the default is 30deg, a hard pan in stereo) |
| `chop(8)` | Cut each sample note into 8 pieces, each playing the next part of the sample |
| `interleave_chops(4)` | Play the pattern once for each of 4 parts of the sample, in turn |

`pan` and `gain` also work as lanes (`pan: [-30deg 30deg]`). Chopping a note whose instrument isn't a sample reports that it has no `begin` parameter when the note plays.

## Signals

`sine`, `saw`, `tri`, `square`, `rand` and `perlin` are patterns with no events of their own: they have a value at every moment, from 0 to 1. Scale them with `range`, and use them as a clip lane (the value at each note's start) or cut them into steps with `to_steps`:

```nml
lead.play([c3 e3 g3 b3].set(bright: sine.range(0.2, 0.9)))
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
