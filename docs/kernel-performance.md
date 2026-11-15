# Interpreted kernel optimization

The kernel compiler optimizes user-written `.nml` without requiring source changes.

The latest default scalar fast path and compile-time fusion measurements are in
[Interpreter scalar fast path](interpreter-performance.md).

## Changes

- Identical scalar sample expressions reuse a register within a safe region.
  Constants are shared by their exact bits, preserving signed zero and NaN
  payload distinctions. Reset and frame boundaries restrict expression reuse.
- Unused scalar math is removed transitively. Native calls, state updates,
  mutable-array operations, and frame work remain in order, even when their
  result is unused. There is no floating-point reassociation or relaxed math.
- Dynamic branches can skip their untaken sample work. A small cost check leaves
  cheap selections branchless. Initializers and block-rate calculations keep
  their existing schedules. Guards for native calls and writes are retained.
- Branch skips are resolved after latency compensation, and dead-code cleanup
  remaps their instruction targets. Kernels with frames or nonzero native latency
  retain unconditional branch histories; optimizing those requires a separate
  control-flow-aware latency analysis.

Value-producing `if` expressions now merge assignments using the same rules as
statement `if` expressions. A new test exposed that scalar assignments in their
branches were not previously merged correctly. The test changes the selected
branch every few samples and checks scalar state together with ordered buffer
writes. Unchanged function, buffer, and lane bindings remain intact when merging.

The subsequent interpreter pass shares identical sample expressions within safe
branch regions and removes redundant boolean tests. It combines conditional
buffer writes and adjacent compare-and-skip instructions. The dispatch loop
keeps common scalar work inline and moves uncommon instructions to a separate
handler. Expensive pure scalar math can use a per-voice cache keyed by the exact
bits of its input operands. Each cache entry covers one operation. Rapidly
changing modulation can miss on every sample.

See [the JIT and AOT estimate](kernel-jit.md) for the machine-code tradeoffs.

## Standard library benchmark

```sh
cargo run --release -p niminal-server --example bench_std -- [seconds] [trials] [filter]
```

The benchmark reports median ns/frame and the percentage of one core at 48 kHz.
Instruments and drums are individual voices at default controls. Effects include
a signal source and the server session's rendering overhead. Blocks use
representative inputs, including a table for `texture` and a repeating
record/play cycle for `splice_reel`. The optional filter matches workload names.
The timing guards in `niminal-server/tests/std_budget.rs` use the same workloads
and broad ceilings to catch large regressions.

### Historical interpreter results

These measurements came from the original interpreter pass on the earlier
shelf, where `splice_reel` was implemented in `.nml`.

| Workload | Before ns/frame | After ns/frame | Approximate change |
| --- | ---: | ---: | ---: |
| splice_reel | 17,200 | 8,400–9,300 | −45% |
| row_sequencer | 443 | 287–306 | −31% |
| low_pass_gate | 204 | 162 | −21% |

The slowest workloads before that pass were `splice_reel` (82% of one core),
`electric_piano` (5.8%), `macro_voice` (5.6%), `overdrive` (3.9%),
`harmonic_drone` (3.8%), and `kick_808` (3.2%). Most presets showed changes
within the roughly ±5% run-to-run variation. In the reported electric piano
profile, the kernel interpreter used about 30% of render time, the envelope
opcode 35%, and oversampling 20%.

The current shelf runs `splice_reel` through native DSP. Current measurements
include recording and playback and use a separate baseline.

### Current shelf

Three alternating release runs on an Apple M1 Pro measured all 110 workloads
with five 0.15-second trials each. The table gives the median of trial medians
in ns/frame.

| Workload | Baseline | Full patch |
| --- | ---: | ---: |
| row_sequencer | 461.3 | 285.8 |
| low_pass_gate | 180.7 | 158.6 |
| splice_reel | 157.9 | 156.7 |
| electric_piano | 1,162.9 | 1,186.1 |
| macro_voice | 1,138.0 | 1,131.7 |
| harmonic_drone | 787.5 | 770.5 |
| kick_808 | 663.8 | 700.1 |

The full shelf sweep flagged `overdrive` for a focused recheck. Five
alternating runs of seven 0.4-second trials measured 834.4 ns/frame at baseline
and 830.4 ns/frame with the patch. Sampling profiles spent most of the render
time in the native oversampler and distortion path.

## Frozen .nml benchmark

The benchmark uses the frozen `.nml` implementations in
`tests/fixtures/blocks_dsp_reference.nml`, through ordinary instrument calls.
The `reference_ns` column measures interpreted DSP. The `native_ns` column
measures the Rust implementation of the same module. The reel case uses
`tests/fixtures/splice_reel_reference.nml` and alternates recording with
pitched, overlapping grains.

### Dispatch and compiler comparison

Three alternating release runs on an Apple M1 Pro used five trials of 1,024
64-frame blocks per build at 48 kHz. These are medians of trial medians in
ns/frame. The baseline is `7f0ef376`. The dispatch column changes only the
interpreter loop. The last column includes the compiler passes and coefficient
cache.

| Frozen workload | Baseline | Dispatch only | Full patch |
| --- | ---: | ---: | ---: |
| contour | 121.5 | 117.6 | 85.9 |
| slew | 111.2 | 110.1 | 83.7 |
| filter steady | 131.9 | 129.6 | 118.1 |
| filter modulated | 203.6 | 201.3 | 177.5 |
| filter stepped | 197.3 | 195.0 | 143.0 |
| frequency shifter | 103.9 | 103.9 | 104.1 |
| reverse | 251.6 | 233.9 | 203.4 |
| tap delay 4 | 1,247.7 | 1,146.9 | 1,130.0 |
| tap delay 16 | 3,484.9 | 3,231.1 | 3,358.9 |
| splice reel | 5,492.3 | 4,791.4 | 3,844.6 |

The dispatch change consistently helped the frozen reel, reverse, and tap
delay. The full patch reduced frozen reel render time by 30%. The 16-tap delay
was fastest with the dispatch change alone. Standalone binary and comparison
instructions still select their operation inside the instruction handler.

### Coefficient cache

In a separate A/B with only cache emission toggled, each build rendered three
rounds of five 1,024-block trials. The stepped filter's cutoff stays constant
for many samples at a time. The modulated filter's cutoff follows a sine wave.

| Frozen workload | Cache off ns/frame | Cache on ns/frame |
| --- | ---: | ---: |
| filter stepped | 193.9 | 146.3 |
| filter modulated | 199.8 | 188.3 |
| slew | 111.1 | 84.2 |
| frequency shifter | 103.0 | 103.7 |

### Earlier scalar and branch optimizations

The pre-change release executable was saved before editing the compiler. Both
executables were run in three alternating rounds, each with five trials of
4,096 blocks (262,144 frames) and 256 warmup blocks. The figures below are the
median of each executable's three trial medians. Compilation and voice creation
are excluded. Machine: Apple M1 Pro, 48 kHz, 64-frame blocks, Rust 1.99.1,
release profile with `-C target-cpu=native`.

| Frozen `.nml` workload | Before ns/frame | After ns/frame | Raw speedup | Native control ns/frame, before → after |
| --- | ---: | ---: | ---: | ---: |
| contour | 257.2 | 126.6 | 2.03x | 24.2 → 24.1 |
| slew | 122.4 | 123.0 | 1.00x | 25.5 → 26.9 |
| filter steady | 143.7 | 141.7 | 1.01x | 48.3 → 47.8 |
| filter modulated | 245.5 | 210.2 | 1.17x | 66.6 → 61.7 |
| frequency shifter | 122.9 | 105.6 | 1.16x | 37.2 → 34.9 |
| reverse | 469.2 | 253.7 | 1.85x | 27.5 → 22.8 |
| tap delay 4 | 1804.2 | 1317.7 | 1.37x | 234.3 → 226.2 |
| tap delay 16 | 4440.1 | 3791.1 | 1.17x | 715.2 → 730.6 |

The native control timings show desktop/CPU variability, particularly for reverse
playback and the modulated filter. The raw ratios are measurements of these runs,
not guaranteed compiler gains. Contour and tap delay show clear improvements;
steady slew and the steady filter show no meaningful change.

For another before/after measurement, build and save the example executable
before changing the compiler, rebuild it afterward, then alternate executions:

```sh
cargo build --release -p niminal-lang --example bench_blocks
cp target/release/examples/bench_blocks /tmp/bench-blocks-before
# Apply the compiler change, then rebuild the same example.
cargo build --release -p niminal-lang --example bench_blocks
/tmp/bench-blocks-before 4096 5
target/release/examples/bench_blocks 4096 5
```

Compare `reference_ns` between builds. The within-build `speedup` column compares
interpreted `.nml` against native Rust and is not the compiler speedup.

## Verification

Focused tests cover shared/unused expressions, exact skip targets after pruning,
cheap selections, per-sample branch changes, scalar-state merging, ordered
buffer updates, independent stateful calls, callbacks, resets, frame latency,
numeric guards, and allocation-free rendering. The frozen DSP comparison also
runs through graphs and stateful functions at 44.1, 48, and 96 kHz.

`scripts/check.sh --test` passed workspace Clippy with warnings denied and
1,872 tests, including the audio baselines, real-time allocation checks, and
standard library timing guards.
