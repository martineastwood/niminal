---
title: Samples and spectral
description: Tables, samples, kits, playback, FFT, triggers, and stutter.
---

## Tables, samples, kits, buffers

| Declaration | What it is | Callable |
| --- | --- | --- |
| `table` | Immutable data: wavetables, curves, IRs | No |
| `sample` | Playable audio + metadata; or multisampled instrument | Yes |
| `kit` | Folder of samples by name | Its samples are |
| `buffer` | Writable data; audio buffers play like samples | Audio buffers are |

```nml
table saw8   = harmonics([1, -1/2, 1/3, -1/4], size: 4096)
table shaper = curve([0 0.2 0.7 1], size: 1024)
table hall   = file("irs/hall.wav")

sample kick  = "drums/kick.wav"
sample hh    = "drums/hh_open.wav" with(choke: hats)
sample amen  = "breaks/amen.wav" with(beats: 4)
sample piano = sfz("keys/piano.sfz")
kit drums    = "drums/808"
```

`kick.data` exposes the underlying table. Kit folders support indexed or patterned variations (`snare.2`, variation lanes on clips).

## Playing samples

Parameters are playback settings:

- **`rate:`** or **`pitch:`** (not both); **`start:`** / **`end:`** in time, `%`, or beats; **`reverse:`**; **`interp:`**.
- **Loops:** `loop: 1.2sec..2.8sec`, `loop_mode:`, `crossfade:`; `loop: from_file` reads metadata.
- **Tempo-synced:** `fit: rate`, `stretch`, or `slice` with `align: bar`.
- **`amen.slices(16)`** — callable, patternable slices.
- **SFZ** chooses zone from pitch and layer from gain.

```nml
kick(pitch: -2st)
snare(pitch: +2st).hpf(cutoff: 200hz)
out = amen(fit: slice, align: bar)
piano(freq: c4, gain: -9db)
```

Samples load off the audio thread; evaluation waits until loaded. **`with(stream: true)`** streams long files. Files are watched and reloaded.

**Live recording:** buffers — `take.record(input(1..2), when: key("r"), align: bar)`, then `take(loop: forever, align: bar)`.

## Spectral processing

FFT yields a **`spectrum`** stream (`spectrum[2048, 512]`). Only spectral opcodes accept it; **`.ifft`** returns to audio.

```nml
instr shimmer(freq: hz) {
  out = osc(saw, freq)
    .fft(size: 2048, hop: 512, window: hann)
    .pitch_shift(+12st)
    .blur(time: 300ms)
    .ifft
}

out = src.fft(size: 2048)
  .map_bins(it.gain(if it.freq < 2khz { 0db } else { -14db }))
  .ifft
```

Built-ins include `pitch_shift`, `freeze`, `blur`, `spectral_gate`, `morph`, `cross`, `spectral_mask`, `map_bins` (bins have `freq`, `mag`, `phase`, `index`).

Fixed-length **`array[f32, N]`** with element-wise ops, `.map`, `.sum`, indexing. Commas stack channels; spaces sequence steps in bin data.

## Triggers and stutter

Patterns become sample-accurate triggers with **`.trig`**.

```nml
fills = grid[.... .... ..xx xxxx]

out = drums.stutter(
  on:    fills.trig,
  size:  [(1/16 beat) (1/32 beat) (1/64 beat) (1/128 beat)],
  pitch: env[0st 1beat +12st],
)
```

See [Built-in opcodes](/reference/opcodes/) for the full v1 list.
