---
title: Units and rates
description: Typed units, conversions, ranges, and signal rates in niminal.
---

## Units

Strict units, free rates: anything can be patched into anything; the scale factor states the meaning (like an attenuverter on a patch cable).

- **Rates never block a connection.** Audio into a control parameter upgrades it to audio rate.
- **Raw signals are unitless** (oscillators, noise, envelopes, LFOs). Unitless × hz = hz; hz + hz = hz; hz + db is an error.
- **Unit suffixes are unit methods:** `800hz` is `800.hz`; `noise().abs.hz` attaches a unit.
- **Related units convert:** note names to hz, db to linear gain, beats to seconds at the current tempo, semitones applied to a frequency.
- **Unrelated units are an error.** Strip with `.num` to reinterpret deliberately: `(-6db).num.hz`.
- **Bare literals in unit slots are errors** with a quick fix (`cutoff: 800` → did you mean `800hz`?). **Exception:** gain parameters accept unitless linear factors (`0.5`) as well as decibels.
- **Unit names** (reserved, at least two characters): `sec`, `ms`, `samples`, `hz`, `khz`, `db`, `st`, `deg`, `bpm`, `beat`/`beats`, `bar`/`bars`, plus `%`.
- **Attaching units:** on literals (`440hz`, `4bars`), including rationals (`1/8beat`). Outside brackets, a space is fine (`4 bars`). Inside pattern, envelope, line, and tracker brackets, a space separates steps — attach the unit or parenthesize: `(1/8 beat)`.
- **Ranges** use `..`: `20hz..20khz`. A range as a parameter type (`bright: 0..1`) declares a bounded unitless parameter.

```nml
lpf(cutoff: 800hz + osc(sine, 110hz) * 600hz)    // audio-rate modulation
lpf(cutoff: lfo(1/4 beat).range(200hz..4khz))    // mapped range
lpf(cutoff: noise().abs.hz * 8000)               // attach a unit
```

## Rates

Signals run at **init** (once per note), **control** (once per block), **audio** (every sample), or **spectral** (once per FFT frame) rate. The compiler infers rates.

Force a rate with a method: `x.init`, `x.control`, `x.audio`.

A parameter fixed at note start declares init before its type:

```nml
length: init sec
```
