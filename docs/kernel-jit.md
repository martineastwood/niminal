# Machine code for .nml kernels

> This note is the estimate made before building anything. The measured outcome, and the decision
> that followed from it, is in [wasm-kernel.md](wasm-kernel.md).

Keep the interpreter as the default. If a representative user patch is limited
by scalar kernel work, prototype one optional JIT path for that patch and compare
it with the interpreter at the same voice count. Use AOT for the same experiment
if the workload is a fixed export or a shipped shelf function. Live edits would
still need an interpreter or runtime compilation.

## Estimated render cost

Using the reported estimate of 2.7 ns per executed instruction, 100 scalar
instructions per sample take about 270 ns/frame, or 1.3% of a core at 48 kHz.
If machine code makes that portion 2–4× faster, it would take 68–135 ns/frame.
These are modeling assumptions. Register residency, branches, state accesses,
math library calls and the machine-code entry cost determine the actual result.

The whole-voice speedup is `1 / ((1 - p) + p / s)`, where `p` is the fraction of
render time eligible for code generation and `s` is its speedup.

| Workload | Assumed eligible share | Whole workload at 2× | Whole workload at 4× | Theoretical ceiling |
| --- | ---: | ---: | ---: | ---: |
| Electric piano, using the reported interpreter profile | 30% | 1.18× | 1.29× | 1.43× |
| Interpreter-heavy user patch, illustrative | 70% | 1.54× | 2.11× | 3.33× |

The electric piano's interpreter share is an upper estimate of eligible work.
The envelope and oversampling account for about 35% and 20% of the profile
respectively. Measure an envelope improvement independently. The shipped reel
already uses native DSP, so the historical interpreted reel is useful as a
compiler stress case and its old timings do not predict savings in Stage.

## Prototype

Start with a scalar loop that includes branches and scalar state. Put the loop
over audio samples inside the generated function so the runtime enters it once per
block. Reuse the existing guarded math helpers for operations whose floating-point
behavior matters, and retain the interpreter for unsupported instructions. Use
Cranelift's JIT module to generate and finalize code before publishing the
definition to the renderer. Keep the owning module alive until the last voice
using its code has stopped. [Cranelift JIT module](https://docs.wasmtime.dev/api/cranelift_jit/struct.JITModule.html)

Measure compile latency from an edit to a ready definition, steady render
ns/frame across repeated alternating trials, and per-block tail latency. Start
with a frozen interpreter-heavy kernel and a modulation-heavy control workload.
Check bitwise audio parity and zero allocations during rendering. A reasonable
prototype gate is at least 25% lower end-to-end render time on an interpreter-heavy
patch, with a reproducible advantage over interpreter improvements. Use that
threshold to decide whether to expand the prototype.
