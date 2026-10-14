# CPU review — 14 October 2026

The array upgrade did not measurably increase CPU for the baseline synth or stateful filter. The review found and fixed a source of wasted DSP: generated nodes continued running even when their values were unused. Larger arrays and function expansion made this waste more expensive.

## Change

`GraphBuilder::build_channels` now keeps only nodes reachable from audio outputs, bus sends, and operations with observable side effects. It compacts both node results and constants and remaps their slots. `TapOut` declares its side effect; kernels propagate that property from embedded operations, so OSC taps remain active even without an audio consumer.

Unused arrays, unused function calls, and discarded elements selected with a constant index no longer add running graph nodes. Dependencies needed by changing indices remain live. This optimization happens while compiling; it adds no audio-thread allocations or processing work.

## Measurements

Release builds on the same machine, 48 kHz stereo. Offline timings are median **user + system CPU seconds** across three renders of 30 seconds of audio, including compilation, the release tail, and WAV writing. Lower is better.

| Fixture | Before pruning | After pruning |
| --- | ---: | ---: |
| Plain saw + native low-pass, limiter on | 0.149 s | 0.142 s |
| Saw + stateful one-pole function, limiter on | 0.159 s | 0.157 s |
| Unused 128-oscillator bank + audible sine, limiter on | 0.990 s | 0.139 s |
| Unused bank + sine, limiter off | 0.917 s | 0.043 s |

The unused-bank fixture uses about **86% less CPU with the limiter**, and 95% less without it. Its rendered WAV is byte-for-byte identical before and after pruning (SHA-256 `1eca8b1a51c0ecf506abbf05c2123eb32a1cdec10dd558e97d64b24d0ff18735`).

A separate comparison against the pre-array commit `d950f75` gave 0.145 s versus 0.142 s for the native synth and 0.156 s versus 0.157 s for the stateful filter. These differences are too small to establish a general regression.

Short headless daemon checks (`--no-audio --no-log`, eight seconds, including startup) measured approximately 2.0% CPU idle before pruning, versus 3.5% for the October 10 commit `13444ee`. The unused-bank case fell from about 14.6% to 1.1% after pruning. These short runs are illustrative; they do not measure sound-card callback behavior or establish the cause of a different project's CPU increase.

A macOS sampling profile of the unused-bank case placed most DSP samples in oscillator processing and the multiplications feeding those unused oscillators. The planner spent most sampled time parked; the master limiter was a smaller contributor in this workload.

## Reproduce the wasted-work case

Save this as `/tmp/niminal-unused.nml`:

```nml
config { channels: stereo }
instr tone(freq: hz) {
  unused = (0..<128).map(fn(i) { osc(saw, freq * (1 + i * 0.001)) })
  osc(sine, freq) * 0.1
}
tone(freq: 220hz) for 30sec
```

```sh
/usr/bin/time -p niminal render /tmp/niminal-unused.nml --out /tmp/niminal-unused.wav
/usr/bin/time -p niminal render /tmp/niminal-unused.nml --out /tmp/niminal-unused.wav --no-limiter
```

Repeat each command and compare `user + sys`, using an optimized release build. To measure the plain synth, replace the body with `osc(saw, freq).lpf(cutoff: 2khz) * 0.1`. To measure the stateful filter, use the same saw with:

```nml
fn lowpass(x, cutoff: hz) {
  state y = 0
  a = exp(-2 * pi * cutoff / sample_rate)
  y = x * (1 - a) + y * a
  y
}
```

Regression tests check that unused array elements do not add running nodes, that result slots remain correct after pruning a preceding multi-output node, and that bus sends and direct or nested OSC taps remain live. The array allocation test also exercises the optimized graph.
