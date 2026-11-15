# Interpreter profiling and envelope fast path

## Where the time goes

An optional `kernel-profile` build counts executed instructions and adjacent
instruction pairs, separately from all timing runs. Counts are thread-local;
the default build contains no profiling hooks. For example:

```sh
cargo build --release -p niminal-lang --example bench_kernel --features kernel-profile
NIMINAL_KERNEL_PROFILE=1 target/release/examples/bench_kernel 2048 7
```

The profile separates interpreted DSP from native opcodes. A CPU sample of the
current electric piano attributed about 33% of main-thread samples to envelope
processing and its block adapter, 28% to oversampling, and 28% to the custom
kernel executor. These are sampling estimates, not precise exclusive timings
for every instrument. Its oversampled section includes parameterized envelopes,
phase-modulated oscillators and final nonlinear saturation.

Representative executed instructions per base-rate frame (262,144 frames):

| Workload | Instructions/frame | Main instruction types |
| --- | ---: | --- |
| electric piano | 92.00 | multiply, native calls, input |
| frozen contour | 33.74 | select, compare, state reads/writes |
| frozen splice reel | 1324.63 | multiply, buffer reads, compare |
| row sequencer | 115.00 | compare, select, multiply |
| scalar cascade | 39.00 | multiply/add fusion |

Instruction counts identify candidates; CPU sampling and uninstrumented timing
are needed to decide whether reducing them actually helps.

## Retained change

Parameterized envelopes implement the existing single-sample opcode interface.
This avoids constructing slices for a one-sample block call inside a custom
kernel. Both entry points share note-start parameter capture and use the same
step and release routines. Fixed envelopes retain their existing block adapter:
a blanket fast path slowed the kick benchmark by about 7% in a longer focused
check, despite helping piano. The selective implementation avoided that penalty
in the subsequent 19-workload sweep.

The 2× oversampling, filter, envelope timing and output are unchanged. Removing
oversampling or moving envelopes outside it is a separate audio-quality change.
Oversampling helps suppress aliasing from nonlinear waveshaping; see
[the JAES study of oversampling filters](https://research.aalto.fi/en/publications/oversampling-for-nonlinear-waveshaping-choosing-the-right-filters/).

## Rejected experiment

Combining adjacent state loads and stores into paired instructions produced
roughly 1–3% gains on several interpreted workloads. This was too small to
justify new VM instructions and extra optimizer cases. The prototype was
removed. It is not part of the envelope measurements in the final CSV.

## Measurement and verification

Use the same release benchmark and alternating-round methodology described in
[interpreter-performance.md](interpreter-performance.md). A filter can select a
focused workload, e.g. `scripts/bench-kernel.py --filter "electric piano"`.
Source compilation is timed separately and no per-note compilation is added.

Tests compare single-sample and block envelope output bit-for-bit across sample
rates, curves, dynamic note-start fields, zero-length segments, looping,
release boundaries and reset. Empty block calls must not consume initial
parameters. The full workspace checks also cover allocation-free rendering,
limiter ceilings, numeric guards, stability and existing audio baselines.
