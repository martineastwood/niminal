# Native modular DSP

`contour`, `slew`, `multimode_filter`, `freq_shifter`, `freeze_reverse`, and
`tap_delay` keep their `std/blocks` interfaces. Thin shelf functions supply
musical defaults, enums, channel handling, and named outputs; their DSP state
and sample processing now live in `crates/niminal-engine/src/ops/blocks/`.
`splice_reel` is unchanged.

The native implementations cache stable coefficients and own their integrators,
read heads, rings, and mode transitions. Tap delay consumes live per-tap control
arrays through one opcode; the compiler checks array lengths and units before
rendering. There are no new dependencies.

## Reproducing the benchmark

```sh
cargo run --release -p niminal-lang --example bench_blocks -- 8192 7
```

The benchmark compiles both the frozen pre-migration shelf and the current shelf
in one executable. It uses one mono voice at 48 kHz, 64-frame blocks, 256 warmup
blocks, and seven trials of 524,288 frames per implementation. It alternates
execution order and reports median nanoseconds per rendered frame. Compilation,
voice construction, and warmup are excluded. The measured work includes the
source oscillator and output arithmetic as well as the module. These are
ordinary instrument calls; timings inside a larger fused kernel may differ.

Machine: Apple M1 Pro (arm64), Rust 1.99.1, release profile with the repository's
`-C target-cpu=native`. Timing results are informational, not test assertions.

## Measured results

Seven-trial medians after the workspace check completed, on 2026-11-10:

| Workload | Original ns/frame | Native ns/frame | Speedup |
| --- | ---: | ---: | ---: |
| Contour | 249.7 | 23.6 | 10.58x |
| Slew | 119.5 | 25.3 | 4.72x |
| Multimode filter, steady controls | 132.2 | 44.1 | 2.99x |
| Multimode filter, modulated cutoff | 206.0 | 60.0 | 3.43x |
| Frequency shifter | 104.5 | 34.9 | 3.00x |
| Reverse playback | 388.3 | 21.8 | 17.79x |
| Tap delay, 4 taps | 1623.9 | 224.5 | 7.23x |
| Tap delay, 16 taps | 4523.9 | 730.8 | 6.19x |

These figures describe the measured patches, not a guarantee for every patch or
machine. All migrated workloads were faster in this run.

## Correctness

`blocks_native.rs` compares the migrated modules against the frozen `.nml`
implementations at 44.1, 48, and 96 kHz, both in graphs and inside stateful
functions. It covers mode transitions, modulation, stereo filtering, and contour
pulses. Tests also cover zero-time linear slew, invalid control arrays, compatible
state transfer, reset, extreme and non-finite inputs, and allocation-free rendering
and reset scopes.

Linear slew with zero time now follows immediately even for changes greater than
one unit. The previous implementation incorrectly limited these to one unit per
sample.

The first unblessed example-baseline run measured maximum block-RMS changes of
0.000029 in `modulation.nml` and 0.000008 in `rain.nml`, with digest energy changes
below 0.001 dB. These examples use the migrated DSP. Native f32 port boundaries
introduce small rounding differences relative to arithmetic inside the original
f64 kernels. The audio references were refreshed after measuring these changes;
no baseline tolerances were changed.

Validation: `scripts/check.sh --test` passed, including workspace Clippy with
warnings denied and 1,855 tests.
