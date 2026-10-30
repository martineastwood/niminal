---
title: Spectral processing
description: Peak-locked pitch shifting, attack handling, independent formants and spectral memory.
---

**Compiled by niminal.** These processors use overlapping Hann-windowed FFT frames. They work in instruments or persistent effects, with latency declared to the graph so parallel paths align. All working storage is allocated before audio processing.

## Pitch, phase locking and attacks

```nml
use std/spectral
instr colour(freq: hz, shift: -48..48 = 7, formant: -48..48 = 0) {
  source = osc(saw, freq) * env[0 10ms 1 | 100ms 0] * 0.1
  out = spectral_pitch(source, shift: shift, formant: formant, preserve: 1)
}
```

`spectral_pitch` wraps the native `spectral_warp` opcode, which takes the same controls. It identifies spectral peaks, assigns surrounding bins to their nearest peak, and translates each region as a group. Overlapping regions add together, rather than overwriting or skipping narrow partials during downward shifts. Peak phases accumulate the measured instantaneous frequency; surrounding bins retain their phase relative to the peak. Regions move by integer bin offsets, while phase accumulation provides continuous pitch between bin centres. Peak tracks match the nearest previous peak within four bins; rapidly moving or newly appearing peaks can restart.

| Control | Default | Meaning |
| --- | --- | --- |
| `shift` | `0` | Pitch shift in semitones, clamped to −48…+48. Duration is unchanged. |
| `formant` | `0` | Independent shift of the broad spectral envelope in semitones, −48…+48. |
| `preserve` | `0` | At 0 the envelope follows pitch; at 1 its original frequency positions are retained. Intermediate values partly move it with pitch. `formant` applies on top. |
| `transients` | `1` | Phase-reset strength on detected attacks, from 0 to 1. |
| `size` | `2048` | Compile-time FFT size: a power of two from 4 to 65536. Adds exactly this many samples of latency. |
| `overlap` | `4` | Compile-time overlap: 2, 4, 8 or 16, no larger than size. Hop is `size / overlap`. |
| `detail` | `32` | Compile-time positive integer controlling cepstral envelope detail. Larger retains narrower features. |

Attack handling uses positive spectral flux: a frame is an onset candidate when growth exceeds 25% of its magnitude sum. Only peak regions whose own growth exceeds 35% are reset. `transients: 1` uses the current analysis phase there; intermediate values move partway along the shortest phase arc. Stationary regions keep integrating. This is a selective reset, not transient bypass or perfect attack reconstruction. Larger FFT windows can still smear attacks; smaller windows separate nearby frequencies less well.

Envelope control uses a smoothed log-magnitude envelope and applies the ratio between desired and naturally shifted envelopes. The log floor is `size * 0.000001`, and correction gain is capped at 32× per bin. It reshapes existing energy; it cannot invent missing harmonics or perfectly model a vocal tract. Leave output headroom, particularly when peak regions collide or formants are boosted. Frequencies outside the real output spectrum are discarded rather than folded back. Stereo channels have separate analysis and peak histories.

Examples:

- `examples/spectral_pitch.nml`: dry, neutral, octave down, fifth up and octave up.
- `examples/spectral_formants.nml`: pitch with a moving or preserved envelope, then independent colour changes.
- `examples/spectral_transients.nml`: repeated attacks through one running processor, with resets off and on.

## Spectral memory

`spectral_memory(x, record: 1, position: 0, frames: 256, size: 2048, overlap: 4)` records a chronological ring of analyzed frames. While `record > 0.5`, it records and processes live input. When recording stops, the capture stays fixed and `position` selects a point from **0 (oldest)** to **1 (newest)**. An empty capture is silent.

Intermediate positions interpolate magnitudes, with magnitude-weighted instantaneous frequencies so empty bins do not detune audible partials. Phases are initialized from the selected capture, then integrated during resynthesis. Holding a position sustains its partials; it does not repeat a frozen phase image. Moving backward scans the captured evolution backward while the partial oscillations keep running forward. To loop a region, feed a repeating position ramp scaled to the desired range.

```nml
instr captured() {
  source = osc(saw, env[110hz 350ms 440hz]) * 0.1
  recording = env[1 350ms 1 1ms 0]
  scan = phasor(1hz).range(0.25..0.65)
  out = spectral_memory(source, record: recording, position: scan,
    shift: 12, frames: 96, size: 1024) * env[1 | 150ms 0]
}
```

Memory also accepts `shift`, `formant`, `preserve`, `transients` and `detail` as above, sharing one analysis/resynthesis pass. Here `transients` defaults to **0**, allowing uninterrupted exploration of stored frames; enable it if local resets suit the captured material. Controls are sampled at frame boundaries. Scanning rapidly or jumping between unrelated spectra can still produce abrupt timbral changes.

`frames` is a compile-time integer from 1 to 65536. History uses `frames * (size / 2 + 1) * 12` bytes; configurations above **64 MiB of history per instance** are rejected. FFT and tracking buffers are additional. At 48 kHz, size 2048 / overlap 4, 256 frames occupy about 3 MiB and successive frame times span about 2.72 seconds. Duration scales with sample rate; the stored frame count stays fixed. Once full, recording overwrites the oldest frame. Reset clears the capture. State-carrying redefinitions preserve it when `size`, `overlap`, `frames` and `detail` match. Prefer a persistent effect for a shared live capture rather than allocating a separate bank per note.

`examples/spectral_memory.nml` demonstrates holding, forward and reverse scanning, and looping a smaller region with independent pitch.
