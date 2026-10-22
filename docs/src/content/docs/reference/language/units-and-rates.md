---
title: Units and rates
description: Typed units, conversions, ranges, and signal rates in niminal.
---

## Units

Strict units, free rates: anything can be patched into anything; the scale factor states the meaning (like an attenuverter on a patch cable).

- **Rates never block a connection.** Audio into a control parameter upgrades it to audio rate.
- **Raw signals are unitless** (oscillators, noise, envelopes, LFOs). Unitless × hz = hz; hz + hz = hz; hz + db is an error.
- **Related units convert:** note names to hz, db to linear gain, beats to seconds at the current tempo, semitones and cents applied to a frequency (`freq + 7st`, `freq + 14ct`).
- **Unrelated units are an error.** Strip with `.num` to reinterpret deliberately: `(-6db).num.hz`.
- **Unit methods** give a plain value a unit: `osc(sine, 2hz).abs.hz * 8000`, `(n * 2).bars`.
- **Bare literals in unit slots are errors** with a quick fix (`cutoff: 800` → did you mean `800hz`?). **Exception:** gain parameters accept unitless linear factors (`0.5`) as well as decibels.
- **Unit names** (reserved, at least two characters): `sec`, `ms`, `samples`, `hz`, `khz`, `db`, `st`, `ct`, `deg`, `bpm`, `beat`/`beats`, `bar`/`bars`, plus `%`.
- **Units are always attached** to a literal: `440hz`, `4bars`, `1/8beat`. Never after a space: inside brackets a space separates steps, and outside them `3/16 beat` would divide by `16beat`.
- **Ranges** use `..` (end included) or `..<` (end left out): `20hz..20khz`, `0..<8`. A range as a parameter type (`bright: 0..1`) declares a bounded parameter.

```nml
lpf(cutoff: 800hz + osc(sine, 110hz) * 600hz)           // audio-rate modulation
ctl cut = midi.cc(21).range(200hz..4khz)                // map a control into hz
lpf(cutoff: (-6db).num.hz * 8000)                       // strip and reattach a unit
```

## Rates

Every value has a rate, inferred as the fastest among its inputs:

| Rate | Changes | Typical uses |
| --- | --- | --- |
| `const` | Never: known while compiling | List sizes, loop counts, a delay's `max`, choices, tables |
| `init` | Once, at note start | A plucked string's length |
| `control` | Once per block | Slow modulation |
| `audio` | Every sample | Signals |

Each value is worked out at its own rate: in a function with state, `exp(-2 * pi * cutoff / sample_rate)` runs once per note if `cutoff` only changes per note.

To read a value more slowly, use a method:

- **`x.init`** is the value `x` had at note start.
- **`x.control`** reads `x` once per block and holds it.

Parameters say which rate they need: `freq: init hz`, `cutoff: control hz`, `size: const int`. See [Functions](/reference/language/functions/).

## Control scopes

Rates are worked out for arithmetic, but anything with state (an oscillator, an envelope, a filter, `state`) runs every sample unless you say otherwise. **`control { ... }`** runs its body once per block instead:

```nml
instr acid(freq: hz) {
  sweep = control { osc(sine, 0.3hz).range(300hz..3khz) }
  osc(saw, freq).lpf(cutoff: sweep.smooth, res: 0.3)
}
```

- **Time is a block's.** Inside the scope `sample_rate` is the control rate (the sample rate divided by `--block-size`), so `osc(sine, 0.3hz)` is still 0.3hz and `env[...]` times are still seconds. A scope's own time steps are one block, so keep it to slow movement: the highest usable frequency is half the control rate (375hz at 64 samples and 48khz), and an envelope segment shorter than a block is a single step.
- **Inputs are read at the start of each block.** A parameter, `ctl` or other value from outside the scope is seen once per block, as with `.control`. In a `fn` with state, only parameters can be read this way and anything else that changes every sample is an error.
- **The result has the `control` rate** and is held through the block. A bare held value steps once per block; where that is audible (a cutoff sweep, a gain) smooth it:

| Written | Result |
| --- | --- |
| `x` | Held: a step each block. The cheapest, and right for slow movement |
| `x.hold` | The same, checked: an error unless `x` changes at most once per block |
| `x.smooth` | A linear ramp from the previous block's value to this block's, in instrument bodies |
| `x.smooth(5ms)` | A one-pole lag at the sample rate. Works in a `fn` with state too |

- **Scopes can appear any number of times** in an instrument, `fn` or node, each with its own state. They can't be inside an `if` or `match` branch of a `fn` with state; put the branch inside the scope.
- **`control fn`** makes a whole function a control scope: `control fn lfo(rate: hz) { osc(sine, rate) }` is `fn lfo(rate: hz) { control { osc(sine, rate) } }`. Every call runs once per block, with state of its own. Its body has one clock, so it can use `state`, branches and any parameters. It has one result; `.smooth` the result at the call site where a step would be heard.
- The sound depends on `--block-size`, because a step lasts a block: the same file at 32 and 256 samples has different step sizes (the frequency of a scope's LFO and its envelope times are the same).

Use `nimlive explain FILE` to see what an instrument runs every sample and what runs once per block, and the editor's rate hints to see the rate of each value in an instrument body. See [Editor and LSP](/reference/editor/).

## Tuning

`ct` is a cent, a hundredth of a semitone: `440hz + 50ct`. Note names are in 12-tone equal temperament with `a4` at 440hz unless the project's `config` says otherwise:

```nml
config {
  tuning: {
    a4: 432hz,                  // where a4 is
    scale: just,                // equal (default), just, pythagorean or meantone
  }
}
```

`scale` also takes your own: `cents(0, 112, 204, 316, 386, 498, 590, 702, 814, 884, 996, 1088)` or `ratios(1, 16/15, 9/8, ...)`, one entry for each note number from the root `c` up to the period, starting at 0ct (or 1). With `period: 1902ct` and thirteen degrees, note names step through Bohlen-Pierce instead of the octave. `a4` stays where `a4:` puts it, whatever the scale. Note names, `freq:` in notes and clips, and key presses all follow it.
