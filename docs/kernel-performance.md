# Interpreted kernel optimization

This pass improves user-written `.nml` kernels. It does not replace their DSP
with Rust implementations or require changes to user code.

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

This does not yet add coefficient caching for unchanged audio-rate inputs,
instruction fusion, direct multi-output native sample calls, or machine-code
compilation.

## Benchmark

The benchmark uses the frozen `.nml` implementations in
`tests/fixtures/blocks_dsp_reference.nml`, through ordinary instrument calls.
The `reference_ns` column measures interpreted DSP; `native_ns` is a control
using the migrated Rust implementation of the same module.

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

`scripts/check.sh --test` passed: workspace Clippy with warnings denied and
1,866 passing tests. Existing audio baselines passed without additional
reference updates for this optimization.
