---
title: Envelopes
description: Envelope literals, sustain, curves, and ADSR.
---

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
- **`adsr(attack:, decay:, sustain:, release:)`** is a named-argument shortcut for the same idea.

## Examples

```nml
env[0 3ms 1.2 40ms 0.8 200ms 0.6 | 100ms 0.3 2sec 0]   // multi-segment, two-stage release
env[0 4beats.log 1 | 2beats 0]                        // tempo-synced swell
cut = env[200hz 10ms 6khz 300ms.exp 900hz | 400ms 200hz]
wob = env[0 (1/8 beat) 1 (3/8 beat) 0].loop
```
