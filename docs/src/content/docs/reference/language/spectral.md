---
title: Spectral processing
description: Spectral morphing, peak-locked pitch shifting, independent freezing, partial tracking and spectral memory.
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

- `examples/spectral_pitch.nml`: a warm phrase, its octave-down bass and fifth-up answer, then a stereo A-minor chord built from shifted copies.
- `examples/spectral_formants.nml`: a held harmony changes apparent size, followed by matched rising phrases with natural and preserved envelopes.
- `examples/spectral_transients.nml`: the same seeded percussion-and-pluck groove, dry and shifted with attack resets off and on.

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

`examples/spectral_memory.nml` records one bowed gesture and reuses that capture throughout: a held detail, forward and reverse scans, then a small looping region harmonized into a minor chord.

## Morph between two sounds

`spectral_morph(x, other, magnitude: 0.5, frequency: 0.5)` analyzes two aligned inputs and interpolates their magnitudes and instantaneous frequencies independently. Each blend ranges from 0 (first source) to 1 (second). Both controls at 0 reproduce the first source; both at 1 reproduce the second, with the declared latency. For example:

```nml
instr changing() {
  a = osc(saw, a3).lpf(cutoff: 1800hz) * 0.12
  b = osc(saw, a3 + osc(sine, 4hz) * 3hz).lpf(cutoff: 4khz) * 0.2
  movement = env[0 1beat 0 4beats 1]
  out = spectral_morph(a, b, magnitude: movement, frequency: movement,
    size: 2048, overlap: 8) * env[0 80ms 1 | 300ms 0]
}
```

This is interpolation between corresponding FFT bins, not matching one sound's harmonics to another's. It is most predictable with overlapping frequency content; unrelated sounds can give deliberately diffuse results. Intermediate frequencies integrate continuously, while frequency endpoints use their source's analysis phases. It is a different transformation from `envelope_transfer`, which transfers the broad spectral shape and retains the carrier's fine structure.

## Morph tracked partials

`partial_morph(x, other, magnitude: 0.5, frequency: 0.5)` tracks both inputs and pairs their tonal components before sine-bank resynthesis. It can glide between widely separated pitches, even when their FFT peaks occupy different bins.

```nml
instr transforming() {
  left = osc(tri, a3) * 0.2
  right = osc(saw, c4).lpf(cutoff: 3khz) * 0.15
  movement = env[0 1beat 0 4beats 1]
  out = partial_morph(left, right, magnitude: movement, frequency: movement,
    count: 24, size: 2048, overlap: 8) * env[0 80ms 1 | 300ms 0]
}
```

| Control | Default | Meaning |
| --- | --- | --- |
| `magnitude` | `0.5` | Linear blend of paired track amplitudes: 0 is the first source, 1 the second. |
| `frequency` | `0.5` | Logarithmic blend of paired frequencies: halfway through an octave is a tritone. Controls clamp to 0…1. |
| `count` | `32` | Strongest peak candidates per source, clamped to `max_tracks`. |
| `threshold` | `0.01` | Peak magnitude relative to each source's current largest FFT bin, 0…1. |
| `freeze` | `0` | Above 0.5, hold both track banks; magnitude and frequency blends remain live. |
| `max_tracks` | `64` | Compile-time track capacity **per source**, a whole number from 1 to 256. |
| `size`, `overlap` | `2048`, `4` | Compile-time FFT settings, with the ranges described above. |

Matching compares each track's frequency relative to its source's lowest currently audible track. For harmonic sources with a detected fundamental, this tends to pair corresponding harmonic numbers. New partners must be within three semitones of that relative position. Existing pairs stay together across changes in loudness and brief detection gaps. Track generations prevent a newly born track from inheriting an old track's partner when a storage slot is reused.

Unmatched components keep their own frequency and fade with their source's magnitude weight. If one source is silent, the other remains available. Each matched pair has one continuously running oscillator; amplitude and frequency targets interpolate over a hop, without restarting its phase when the blend changes. There are at most `2 * max_tracks` oscillator slots. During complete track turnover, retiring oscillators may need to fade out for a hop before all new tracks can sound.

This is **tonal resynthesis**, so endpoints reproduce the tracked approximation rather than the original audio. Noise, breath and attacks are not preserved as a separate residual layer. The lowest-track reference is not a fundamental-frequency detector: missing fundamentals, noisy inputs and polyphonic mixtures can produce less predictable pairings. Begin with clear monophonic sources and leave output headroom. Stereo channels track and pair independently.

All controls are sampled at frame boundaries. Declared latency is `size / 2 + size / overlap` samples, as for `spectral_partials`; extracted attack timing is approximate. Reset clears both banks and all pairs. Compatible live redefinitions preserve the complete capture and oscillator state when size, overlap and capacity match. Audio processing uses only preallocated buffers.

`examples/partial_morph.nml` first auditions a rounded bowed A3 and a brighter, breathing C4. It then moves between their tracked partials, freezes the banks, and brings the matched pitches back toward A while retaining B's held amplitude profile. Compare it with `examples/spectral_morph.nml`, which demonstrates bin-based interpolation.

## Select and arpeggiate spectral components

| Opcode | Controls | Result |
| --- | --- | --- |
| `spectral_thin(x, count: 16)` | `count`: number of bins, rounded down and clamped to 0…`size / 2 + 1` | Keeps the loudest bins in each frame. Zero is silence; keeping every bin reproduces the input. Ties favour lower bin indices. |
| `spectral_arp(x, position: 0.1, depth: 1, gain: 1, width: 1)` | `position`: 0…1 of Nyquist; `depth`: 0…1; `gain`: nonnegative linear gain; `width`: triangular focus radius in bins, at least 1 | Emphasizes a moving area of the spectrum. Depth 1 silences material outside the focus; depth 0 / gain 1 is neutral. |

Animate `count` to dissolve a recording into its strongest components, or step `position` in time with a sequence. A component usually occupies several neighboring FFT bins, so keeping 8 bins does **not** mean keeping 8 harmonics. Thinning is frame-local and can flicker as bins exchange rank. A wider arpeggiator focus gives gentler movement; larger gain requires output headroom.

## Freeze magnitude and frequency independently

`spectral_hold(x, magnitude: 0, frequency: 0)` captures either component when its control rises above 0.5, retaining it until that control falls again. The two captures are independent:

- Hold magnitude to keep the spectral shape while analysis phases follow the live input.
- Hold frequency to keep each bin's measured frequency while live magnitudes continue changing.
- Hold both to sustain the captured spectrum, including off-bin tuning.

Start the freeze after a sound has developed: freezing an empty frame holds silence. Frequency-only holding retains frequencies **by bin**, not by harmonic identity; large pitch movements can move energy into bins whose captured frequencies were unreliable. This opcode complements the existing `spectral_freeze` and the chronological capture in `spectral_memory`.

## Shift frequencies in Hz

`spectral_shift(x, shift: 100hz, lowest: 500hz)` adds a fixed Hz offset to spectral peak regions whose measured peak frequency is at least `lowest`. The default shift and cutoff are both 0 Hz. Negative shifts move regions downward. Whole peak regions move together, with phase locking and attack resets as in `spectral_warp`; bins outside the output spectrum are discarded. Frequencies below the cutoff retain their tuning.

Adding Hz changes harmonic spacing relative to the fundamental, producing inharmonic and bell-like textures. This differs from `spectral_pitch`, which multiplies frequencies to transpose in semitones. Moving regions across the cutoff can change their shift abruptly; use a steady cutoff for predictable results.

## Track and resynthesize partials

`spectral_partials(x, count: 32, threshold: 0.01)` detects local spectral peaks, follows them across frames, and resynthesizes them with sine oscillators. Amplitude and frequency interpolate over each hop. This extracts a tonal approximation; it does not reconstruct the noisy residual or guarantee a transparent bypass.

| Control | Default | Meaning |
| --- | --- | --- |
| `count` | `32` | Maximum strongest peak candidates considered each frame, clamped to `max_tracks`. Zero lets existing tracks fade away. |
| `threshold` | `0.01` | Minimum peak magnitude relative to the current frame's largest bin, 0…1. |
| `shift` | `0` | Transpose tracked oscillators in semitones, −48…+48. Out-of-band oscillators are muted. |
| `freeze` | `0` | Above 0.5, retain the tracked amplitudes and frequencies while oscillators continue. |
| `track` | `0` | 0 plays every track; a positive integer selects a 1-based persistent slot. An empty or out-of-range slot is silent. |
| `lowest`, `highest` | `20hz`, `20000hz` | Accepted measured peak frequencies. |
| `max_tracks` | `64` | Compile-time slot capacity, a whole number from 1 to 256. |

Track slots keep their identity when loudness ordering changes. Matching uses nearest frequency within the greater of three FFT bins or 4% of the previous frequency, with at most one candidate per slot. Missing tracks fade over one hop and can reconnect for three frames before their slots are released. Slots are reused after tracks die; these are bounded live identities, not permanent IDs for an entire recording. Crossing or unresolved partials can still exchange identities. Freeze a capture before selecting slots when you want a stable component to audition.

These spectral tools accept compile-time `size: 2048` and `overlap: 4`, with the same ranges as `spectral_warp`. Controls are sampled at frame boundaries, except `spectral_partials` transposition and slot selection, which are sampled per audio sample. Bin-based morphing, thinning, arpeggiation, holding and Hz shifting add `size` samples of latency. Both `spectral_partials` and `partial_morph` declare `size / 2 + size / overlap` samples for window-centre analysis and one-hop interpolation; extracted attacks are approximate. Stereo channels are processed independently. Working buffers and track storage are allocated before processing, and compatible live redefinitions carry their state.

Examples: `spectral_morph.nml`, `spectral_select.nml`, `spectral_hold.nml`, `spectral_shift.nml` and `spectral_partials.nml` each start with an audible source before introducing their transformation.
