---
title: Tables
description: Compile-time tables, constructors, transforms, readers and shared buffers.
---

**Compiled by niminal.** Tables are built by the compiler and read by the running instrument.

Tables are fixed-length arrays of plain numbers, shared by every voice. They hold wavetable cycles, transfer functions, windows, impulse responses and lookup curves. Samples are tables with playback metadata; shared `buffer` declarations are writable tables that outlive voices.

## Constant tables

`const` evaluates an array while compiling. Prefer a named constructor, or build one with a literal or `.map`:

```nml
const organ = harmonics([1, 0, 1/3, 0, 1/5], size: 2048)
const saw8  = harmonics([1, -1/2, 1/3, -1/4], size: 4096)
const rise  = curve([[0, 0], [1, 1]], size: 1024, shape: exp)
const hall  = file("irs/hall.wav")
```

`osc(organ, freq)` plays a table as one cycle, band-limited at any pitch. A `bank([a, b, c])` morphs between tables with `osc(b, freq, position: 0.5)`.

## Constructors

| Constructor | Role |
| --- | --- |
| `harmonics(amps, size:, phases:)` | Harmonic sine series (GEN10) |
| `partials([[ratio, amp], ...], size:)` | Inharmonic partials |
| `cosines(amps, size:)` | Cosine series |
| `curve([[x, y], ...], size:, shape:)` | Breakpoint curve: `linear`, `exp`, `cubic`, `spline` |
| `steps(xs, ys, size:)` | Step function |
| `poly(coeffs, size:)` | Polynomial over -1..1 |
| `ramp(start, end, size:)` | Linear ramp |
| `cheby(amps, size:)` | Chebyshev transfer function for waveshaping |
| `window(kind, size:)` | `hann`, `hamming`, `blackman`, `kaiser(beta)`, `gauss`, `tukey`, `sinc`, `tri` |
| `random(size, dist:, seed:)` | Deterministic noise; `seed:` is required |
| `file("path.wav", channel:, start:, end:)` | Load one channel of a WAV, or one column of a `.csv`, `.txt` or `.dat` file of numbers (a first line that isn't numbers is a header, `#` starts a comment) |
| `bank([t1, t2, ...])` | Wavetable bank for `osc(..., position:)` |
| `zeros(n)` | Zero-filled array (also used to size buffers) |

A path can hold constants, so a table can be picked by number: `file("kick{n}.wav")`, and `{n:2}` pads to two digits (`kick07.wav`). Each must be a whole number known while compiling.

## Transforms

Methods on a table: `.normalize`, `.reverse`, `.rotate(n)`, `.resample(n)`, `.concat(other)`, `.integrate`, `.mix(other, amount:)`.

## Readers

| Form | Meaning |
| --- | --- |
| `t[i]` | Entry at a wrapping index |
| `t.at(pos, interp:, edge:)` | Fractional read; `interp:` is `none`, `linear` or `cubic`; `edge:` is `wrap` or `clamp` |
| `t.phase(p)` | Read at normalized position 0..1 |
| `x.waveshape(t)` | Map `x` in -1..1 through transfer table `t` |

## Samples and tables

A sample is a table plus playback settings. `kick.data` is the underlying table (one table, or an array of channel tables). A const table can be played as a sample:

```nml
const organ = harmonics([1, 0, 1/3], size: 2048)
pad = sample(organ).root(c4).loop(forward)
```

`loop:` is `none`, `forward` or `bidirectional`.

## Shared buffers

```nml
buffer take = 4bars
buffer scratch = zeros(48000)

instr recorder() {
  take.record(input(1), loop: true)
}

instr scrub(pos) {
  take.at(pos * sample_rate)
}
```

Buffers are declared at the top level. Write with `buf[i] = x` or `buf.record(sig, loop:)`. Read with `[i]`, `.at`, `.phase`, or play via a sample. Functions cannot write shared buffers. Live re-evaluation keeps a buffer's contents when the name, frame count and channel layout are unchanged.

### Buffer resources from a client

The engine API can allocate, inspect, load and save shared buffers without editing instrument source. `buffers.load` loads all WAV channels by default (or one selected channel) and resamples to the engine rate; `buffers.save` writes a multichannel float WAV snapshot. Updating buffer samples uses the timed `buffer_write` event, so writes participate in atomic batches, cancellation and replay. See [Engine API](/reference/engine-api/#buffers).

A buffer's length is fixed. Redefine it to resize it; matching names and lengths preserve contents across live redefinitions. Read/save snapshots may span concurrent audio-thread writes.


Use `buffer stereo = zeros(48000, channels: 2)` for multichannel storage. Positions measure frames. `stereo.at(0, channel: 2)` reads channel two; channel selectors are 1-based. `stereo.record([left, right])` records both channels, or `stereo.record(right, channel: 2)` selects a starting channel. Buffer API values are interleaved. `buffers.free` unpublishes a name while prepared voices retain their storage; a new allocation of the same name is a distinct generation. All retained generations count against the connection’s configurable memory budget (256 MiB by default).
