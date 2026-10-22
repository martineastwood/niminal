---
title: Standard library
description: Filters, physical models, delays, a reverb and spectral effects, written in niminal and shipped with it.
---

The modules under `std/` come with niminal. They are ordinary niminal source, read from the program itself, so `use std/filters` works anywhere and nothing needs installing. They are also the best examples of what [functions](/reference/language/functions/) can do, so read them: they are short.

```nml
use std/filters
use std/reverb

instr pad(freq: hz) {
  out = osc(saw, freq).ladder(cutoff: 900hz, res: 0.4)
  out += out.fdn(time: 2.5sec) * 0.3
}
```

A function from a module is called like any other, as a method or with the signal first. Parameters marked `init` are read when a note starts. The ratios and other tables the functions use are public too (`drum_modes`, `fdn_lengths`, and so on), so you can use them or write your own.

## std/filters

| Function | What it is |
| --- | --- |
| `ladder(x, cutoff, res: 0.3)` | A four-pole low-pass with a `tanh` in each stage, like a transistor ladder. `res` near 1 rings and then sings at the cutoff. Loud input saturates the stages, as it should. |
| `biquad_lp(x, cutoff, q: 0.7071)` | A two-pole low-pass. `q` is the height of the peak at the cutoff; 0.707 is flat. |
| `biquad_hp(x, cutoff, q: 0.7071)` | A two-pole high-pass. |
| `biquad_bp(x, cutoff, q: 1)` | A two-pole band-pass with a peak gain of 1. |
| `biquad_notch(x, cutoff, q: 1)` | Everything except a band around the cutoff. |
| `onepole_lp(x, cutoff)`, `onepole_hp(x, cutoff)` | One pole: 6db an octave. |
| `biquad(x, b0, b1, b2, a1, a2)` | One biquad section from its coefficients, for building your own. |

## std/physical

| Function | What it is |
| --- | --- |
| `string(x, freq: init hz, decay: 0.996, size: 4096)` | A plucked string (Karplus-Strong). Feed it a short, bipolar burst such as `noise() * env[1 4ms 0]`; a click with only positive values gives a string that only drifts. `size` is the longest period, in samples. |
| `modal(x, freq: init hz, ratios, decay: 0.9995)` | A resonator for each of `ratios` times `freq`, in parallel. Keep `freq * ratio` below half the sample rate. |
| `drum(x, freq: init hz, decay: 0.999)` | The first eight modes of a circular membrane. |
| `free_bar(x, freq: init hz, decay: 0.9997)` | A free bar, as in a glockenspiel. |
| `bell(x, freq: init hz, decay: 0.9998)` | A bell, with a hum an octave below the strike note. |

```nml
use std/physical
instr tom(freq: hz = 110hz) { (impulse(1hz) * 0.5).drum(freq) }
```

## std/delays

| Function | What it is |
| --- | --- |
| `multitap(x, times, gains, max: 96000)` | Echoes at several times, from one delay line. `x.multitap([120ms, 250ms, 410ms], [0.6, 0.4, 0.3])` gives `x` plus the echoes. `max` is the longest time in samples. |
| `tape(x, time, feedback: 0.5, damp: 4khz, max: 96000)` | A feedback delay with a low-pass in the loop, so each repeat is darker. |

## std/reverb

| Function | What it is |
| --- | --- |
| `fdn(x, time: init 2sec, damp: init 5khz, size: init 1)` | A feedback delay network: eight delay lines of mutually prime lengths mixed by a Hadamard matrix. `time` is how long the tail takes to fall by 60db, `damp` the cutoff of the low-pass in each line, and `size` (0.5 to 2) stretches the lines. Mono in, mono out, and wet only: mix it with the dry yourself. |

A tail costs about 3% of one core, so one on a bus is cheap.

## std/spectral

These use [`frames`](/reference/language/functions/#frames-and-the-fft), so each is `size` samples late (1024 unless you give `size`); the compiler delays the path beside it to match.

| Function | What it is |
| --- | --- |
| `band_keep(x, lo: 0, hi: 0.25, size: 1024)` | Keeps the part of the spectrum from `lo` to `hi`, as fractions of the way to half the sample rate. |
| `robot(x, size: 1024)` | Keeps every magnitude and zeroes every phase: a buzz at the frame rate. |
| `blur(x, amount: 0.9, size: 1024)` | Mixes each spectrum with the ones before it: a long wash. |
| `denoise(x, threshold: 0.01, size: 1024)` | Silences the bins quieter than `threshold`: a spectral noise gate. |

## Writing your own

Anything in these files can be copied and changed. A module you write goes next to your project or in `lib/`; see [Modules](/reference/language/modules/). If it turns out well, tell us: the library grows by what people need.
