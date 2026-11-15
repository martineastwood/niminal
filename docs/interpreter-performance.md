# Interpreter scalar fast path

The default interpreter handles add, subtract, multiply, guarded division, min
and max directly in its hot dispatch loop. After the existing scalar cleanup,
the compiler also combines adjacent multiply/add pairs into one instruction.
The multiplication still rounds as f64 before addition; this is not FMA.
No JIT, dependencies, or runtime/backend switches are added.

## Safety and scope

Fusion requires single-definition registers, a product with exactly one use,
and no branch entering at the add. Use counts include outputs, state writes,
computed arrays, native inputs, frame results and reset/frame code. Products
observed elsewhere are retained. Removing an instruction remaps forward skip
counts using the existing pruning machinery. Kernels without candidate pairs
skip the analysis, and a pass which finds nothing does not rebuild the code.

The new instruction is understood by liveness, register remapping and latency
alignment. Tests cover branch targets, skip remapping, observed intermediates,
multiple definitions, signed zero, overflow, and a rounding case which would
change under FMA. Finite edge results are compared bit-for-bit; NaNs are compared
by classification because [Rust does not specify arithmetic NaN payloads](https://doc.rust-lang.org/std/primitive.f64.html#nan-bit-patterns).
The existing limiter, numeric-guard, stability and audio-baseline tests are
unchanged.

## Kernel measurements

Apple M1 Pro, Rust 1.99.1, release with `target-cpu=native`, 48 kHz / 64 frames.
Three alternating before/after rounds each use seven 2,048-block trials and
256 warmup blocks. The benchmark uses preallocated `Voice::process_blocks`,
realtime mode, and an advancing engine clock. Current instruments retain their
exposed controls. Timings exclude compilation and voice creation. Builds and
tests were stopped during measurement.

The saved before executable comes from the `dev` experiment checkpoint
`21f4b142`, with its JIT disabled; it includes the interpreter/compiler work
which was already uncommitted when that experiment began. These comparisons
measure the additional scalar changes, not all changes relative to Git main.

| Workload | Before ns/frame | After ns/frame | Speedup |
| --- | ---: | ---: | ---: |
| frozen contour | 84.9 | 86.1 | 0.99× |
| frozen slew | 79.9 | 62.8 | 1.27× |
| frozen filter steady | 114.7 | 98.6 | 1.16× |
| frozen filter modulated | 177.8 | 138.3 | 1.29× |
| frozen filter stepped | 141.9 | 109.8 | 1.29× |
| frozen frequency shifter | 100.0 | 105.3 | 0.95× |
| frozen reverse | 188.0 | 173.0 | 1.09× |
| frozen tap delay 4 | 1041.3 | 941.6 | 1.11× |
| frozen tap delay 16 | 2996.9 | 2561.3 | 1.17× |
| frozen splice reel | 3527.0 | 3250.9 | 1.08× |
| scalar cascade | 254.3 | 181.4 | 1.40× |
| electric piano | 1100.3 | 1087.2 | 1.01× |
| macro voice | 1118.8 | 1115.6 | 1.00× |
| harmonic drone | 761.4 | 751.5 | 1.01× |
| kick 808 | 636.1 | 617.5 | 1.03× |
| low pass gate | 166.7 | 168.7 | 0.99× |
| row sequencer | 275.4 | 271.9 | 1.01× |
| native overdrive path | 479.2 | 474.7 | 1.01× |
| native filter | 23.8 | 25.0 | 0.95× |

Every verification render matched bit-for-bit for all 19 workloads in all three
rounds (262,144 samples per workload and round). The frozen workloads are the
old `.nml` DSP fixtures; the current shelf uses native Rust for several of these.
The scalar cascade is synthetic. Most current shelf instruments show smaller
gains than custom DSP; the tiny native-filter case and frozen frequency shifter
are about 5% slower. Changes of a few percent need focused rechecks.

The combined path was selected after separately measuring direct arithmetic,
compile-time fusion, and their combination. The combination consistently helped
the larger scalar workloads. Unlike the earlier runtime-lookahead experiment,
fusion decisions are made once during ordinary language compilation. There is
no per-note compilation. The CSV includes first and median-of-three source
compilation times and per-block tail timings; desktop timing noise prevents
interpreting maxima as hard realtime bounds.

See [kernel-interpreter.csv](kernel-interpreter.csv) for the final measurements.

## Whole shelf check

The existing `bench_std` executable compared all 110 current block/preset
workloads in three alternating before/after rounds, five 0.3-second audio trials
per process. It measures individual voices/default controls; effects include
a server session. This is a regression sweep, not a complete live-set benchmark.
Its baseline was built with the same opcode representation and fusion disabled.

Most native-heavy workloads were close. The short sweep flagged `flip_flop`:
39.2 → 43.1 ns/frame. Longer alternating trials (five rounds, fifteen trials of
30 audio seconds each) gave 39.6 → 38.7 ns/frame, with overlapping distributions.
The original short-run flag was not a reproducible slowdown. The final sweep
and this focused recheck found no confirmed regression over 5%.

See [std-interpreter.csv](std-interpreter.csv) for the broad sweep. Avoid treating
small changes there as guaranteed gains: process-to-process variance is visible
on tiny workloads.

## Reproduce

Build and save the benchmark before editing, then compare the new build:

```sh
cargo build --release -p niminal-lang --example bench_kernel
cp target/release/examples/bench_kernel /tmp/kernel-before
# Make the compiler/interpreter change.
scripts/bench-kernel.py --reference /tmp/kernel-before
```

The driver builds the current executable before timing, alternates executable
order, saves raw CSVs/executable snapshots, and rejects differing audio bytes.
Use `--blocks`, `--trials`, `--rounds`, or `--output` to adjust the run.
Without `--reference` it measures the current build alone.

The broader shelf sweep is available separately:

```sh
cargo run --release -p niminal-server --example bench_std -- 0.3 5
```

## Validation

`scripts/check.sh --test` passed workspace Clippy with warnings denied and all
1,877 tests, including limiter ceilings, numeric guards, stability, real-time
allocation checks, audio baselines and tempo-ramp note scheduling. No audio
baselines were re-recorded. The existing nine ignored tests were unchanged.
