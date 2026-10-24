---
title: Envelopes
description: Envelope literals, sustain, curves, adsr, and perc.
---

**Compiled by niminal.** An envelope is part of an instrument's code, compiled into the sound.

Envelopes use their own literal: **levels and durations alternate**.

```nml
env[0 5ms 1 120ms 0.4 | 600ms 0]
```

Starts at 0, reaches 1 in 5ms, 0.4 in 120ms, holds at the sustain point (`|`), and on release takes 600ms to reach 0.

## Rules

- Any number of segments; two levels or two durations in a row is an error.
- **`|`** marks the sustain point. No `|` means a one-shot that plays through.
- **Durations** in any time unit, mixed freely. Beat segments follow tempo changes.
- **Curves** are methods on the duration: `900ms.exp`, `5ms.log`, `600ms.curve(-4)`.
- **Levels** can have any unit, as long as they all match: hz, db, or unitless.
- **Releasing mid-segment** starts release from the current level.
- **`.loop`** repeats the shape.
- **Steps can be names or `(expressions)`**, read at note start, so an envelope can follow the instrument's parameters:

```nml
instr pad(freq: hz, attack: sec = 400ms, release: sec = 2sec) {
  osc(saw, freq) * env[0 attack 1 | release 0]
}
```

## Shortcuts

The two shapes most instruments want have names, and each is exactly the literal beside it:

```nml
adsr(attack: 5ms, decay: 120ms, sustain: 0.4, release: 600ms)   // env[0 5ms 1 120ms 0.4 | 600ms 0]
perc(decay: 400ms)                                              // env[0 2ms 1 400ms.exp 0]
perc(attack: 10ms, decay: 1/4beat)                              // env[0 10ms 1 1/4beat.exp 0]
```

`adsr` takes all four, named or in order (`adsr(5ms, 120ms, 0.4, 600ms)`). `perc` is a one-shot attack and exponential decay, with `attack` defaulting to 2ms. Like the literal, they take numbers or names (`adsr(5ms, 120ms, 0.4, release)`). A user function with one of these names is used in preference.

## Examples

```nml
env[0 3ms 1.2 40ms 0.8 200ms 0.6 | 100ms 0.3 2sec 0]   // multi-segment, two-stage release
env[0 4beats.log 1 | 2beats 0]                        // tempo-synced swell
cut = env[200hz 10ms 6khz 300ms.exp 900hz | 400ms 200hz]
wob = env[0 1/8beat 1 3/8beat 0].loop
```
