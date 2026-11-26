# Kernel backends: native code and WebAssembly

`.nml` kernels (`fn` bodies with state) compile to a register-machine IR that the interpreter runs
one instruction per sample. There are now three other ways to run the same IR, all bit-identical
to the interpreter and all checked against it with the same random differential test:

| Backend | Where | Status |
| --- | --- | --- |
| dynasm code generators (`custom/jit/`) | native: aarch64 and x86-64 | the native tier, always built there; on unless `NIMINAL_JIT=0` |
| wasm emitter (`custom/wasm.rs`, feature `wasm-kernel`) | the web playground, in the browser's own engine | emitter works; the browser loader and the wasm32 engine build are not written |
| wasmtime runner (`custom/wasm_run.rs`, feature `wasm-kernel`) | tests and benchmarks only | runs the emitted wasm natively, `NIMINAL_WASM=1` |

The interpreter stays: it runs everything the other backends do not cover (kernels that call a
built-in opcode, frame scopes, vector instructions, reset scopes) and every platform without a
code generator.

## Decision

Native speed comes from dynasm, not from wasmtime. Measured on aarch64 with the same kernels,
dynasm was 1.1–2.5× faster than wasmtime's Cranelift, compiled in 0.01–0.8 ms instead of 1.6–25 ms
(so no interpreter-first switch is needed), and cost about 110 KB of binary instead of about 8 MB
and 40 s of clean build. The wasm emitter stays for the web, where the browser compiles it.
Wasmtime is kept only as a test and benchmark harness for the emitter. The rest of this note is the
evidence: wasmi (an interpreter) and Winch (wasmtime's baseline compiler) were tried and removed.

Every kernel backend must keep bit-identical output: `cargo test -p niminal-engine --features
wasm-kernel` runs the differential tests, and every audio baseline passes with the JIT on, which is
the default (`NIMINAL_JIT=0` is the kill switch).

## Where it stands

The native JIT is merged, built in and on by default on aarch64 and x86-64. CI runs the engine and
language suites through it on Linux x86-64, Apple Silicon, Windows, Linux ARM and Intel macOS (all green);
`cargo test -p niminal-cli --test jit_identical` renders every example with `NIMINAL_JIT=0` and `=1` and
requires the same bytes. `NIMINAL_JIT_VERBOSE=1` lists what was generated and why anything was not, and
`NIMINAL_JIT_DUMP=<kernel name>` prints a kernel's instructions.

- **Coverage.** Kernels that call built-in opcodes are generated (the code calls the interpreter's own
  opcode code through a trampoline). Across the examples 396 kernels are generated; the 27 that are not
  run a frame scope (the spectral functions). Vector instructions (`fir`, `taps`, FFT) are not generated.
- **Against native Rust** (M1 Pro, one mono voice, ns per frame; only contour is bit-identical, the
  others behave the same but are not the same algorithm): contour 20 / 19, slew 14 / 15, the three
  filters 34-43 / 33-40, reverse 19 / 14, tap delay 4 and 16 116 / 191 and 239 / 593, splice reel 269 / 342
  (JIT-compiled `.nml` / Rust). The frequency shifter is still interpreted (about 2.8x slower).
- **List access.** An index is wrapped with one floor conversion and integer compares, and a register
  written and read inside one branch is a temporary that can share an FP register; together they took the
  splice reel kernel from 665 to 269 ns.
- **Write the work inside the `if` arm.** A value computed before an `if` and only chosen by it is
  computed every sample. Moving the expensive part (a list read, `sin`, `exp`) into the arm that uses it
  lets the compiler skip the other arm: the reverse block went from 19 to 17 ns with identical audio.
  The std shelf and the examples have no case of this worth changing.

### Web: what a probe showed

`niminal-engine`, `-lang`, `-compose`, `-pattern`, `-score`, `-server` and `-lsp` all pass
`cargo check --target wasm32-unknown-unknown` unchanged (the JIT is not built there). `-ws`, `-host` and
`-sequencer` do not: `getrandom` needs its `wasm_js` feature (it comes in through the WebSocket stack
and the sound card), and the sequencer depends on both. A probe crate (outside the repository) compiled
the engine and language to a 5.3 MB wasm module and rendered six pieces in Node's V8, 64 blocks each:
five rendered the same audio bytes as the native build, and the sixth (the harmonic drone, which leans on
`sin`, `exp` and `pow`) differed in the last bits (the sum of the samples agrees to 5e-10), the maths
library gap noted above. The engine ran 1.0-1.9x slower than native for opcode-heavy pieces (the wasm side
runs the interpreter, since the JIT does not exist there). Known run-time traps that `cargo check` does
not find: `Instant::now` in `mixer/render.rs` panics on `wasm32-unknown-unknown`, and the sample, module
and preset loaders use the file system and `SystemTime`.

## What it is

- [`custom/wasm.rs`](../crates/niminal-engine/src/ops/custom/wasm.rs) turns a kernel's per-sample
  code into one wasm function that runs the sample loop for a block. It needs only the
  `wasm-encoder` crate, so it has no native-only dependency.
- [`custom/wasm_run.rs`](../crates/niminal-engine/src/ops/custom/wasm_run.rs) runs that module
  natively through wasmtime. A browser would instantiate the same bytes itself.
- Arithmetic, comparisons, selects, branches, state, shared tables, state lists (read, write,
  linear and cubic interpolation) and the coefficient cache (`Memo`) are emitted inline. Registers
  and state live in wasm locals, so the engine's register allocator can keep them in machine
  registers.
- Only kernels made entirely of those instructions compile. A kernel that calls a built-in opcode,
  runs a frame scope or a vector instruction, or has reset scopes, stays interpreted (`NIMINAL_WASM_VERBOSE=1`
  logs why).

Rounding is the interpreter's: no FMA, no reassociation, the same guarded division, Rust's
NaN-ignoring `min`/`max`. Functions that are not wasm primitives (`sin`, `exp`, `pow`...) call the
same Rust helpers through an import.

## Correctness

- `tests/it/wasm_kernel.rs`: 400 random kernels (every inline instruction, nested and overlapping
  skips, NaN, infinities, signed zeros, huge and tiny values, a reset part-way) run through the
  interpreter and wasm for four blocks each. Every register of every sample matches bit for bit
  (NaNs compare as NaN). 
- `tests/wasm_node/check.mjs`: 200 random kernels, limited to operations whose result does not
  depend on a maths library, run in Node's V8 and compared with the Rust interpreter: 0 mismatches.
  A browser engine computes the same bits as the interpreter.
- `tests/wasm_alloc.rs`: 100 blocks render with zero allocations.
- Whole programs: the frozen `.nml` reference DSP and the shelf instruments in
  `examples/bench_wasm.rs` render the same audio hash with the interpreter and with wasm, for all
  16 workloads.

## Measurements

Apple M1 Pro, release, 48 kHz, 64-frame blocks, one voice. `bench_wasm` reports the median of
three trials of 512 blocks; the table is the median of three alternating interpreter/wasm rounds.

```sh
cargo build --release -p niminal-lang --example bench_wasm --features wasm-kernel
NIMINAL_WASM=0 target/release/examples/bench_wasm 512 3 > interp.csv
NIMINAL_WASM=1 target/release/examples/bench_wasm 512 3 > wasm.csv
```

| Workload | Interpreter ns/frame | Wasm ns/frame | Speedup | Compile ms, interpreter → wasm |
| --- | ---: | ---: | ---: | ---: |
| scalar cascade | 168 | 24 | 7.0× | 0.3 → 3 |
| frozen splice reel | 2,467 | 718 | 3.4× | 53 → 225 |
| frozen reverse | 137 | 43 | 3.2× | 10 → 33 |
| frozen slew | 52 | 19 | 2.8× | 9 → 31 |
| frozen tap delay 16 | 2,011 | 751 | 2.7× | 14 → 59 |
| frozen tap delay 4 | 747 | 300 | 2.5× | 11 → 40 |
| frozen contour | 76 | 35 | 2.2× | 10 → 32 |
| frozen filters (steady, modulated, stepped) | 81–110 | 43–61 | 1.8–1.9× | 9 → 31 |
| frozen frequency shifter, electric piano, macro voice, harmonic drone, kick 808, overdrive | 74–482 | same | 1.0× | unchanged (interpreted) |

The audio hash is identical to the interpreter's for all 16 workloads. Per-block p99 falls with
the mean. Compile time is from source to a ready definition; most of the added time is
Cranelift compiling the module.

Cost of carrying wasmtime (`runtime`, `cranelift`, `std` features only), `bench_wasm` example,
release profile, clean build on the same machine:

| | Without | With `wasm-kernel` |
| --- | ---: | ---: |
| clean build | 23 s | 66 s |
| binary size (not stripped: `rust-objcopy` fails on this machine) | 9.4 MB | 20.9 MB |

Where the weight is, from a minimal program that compiles and runs one module (release, clean
build, M1 Pro; stripped sizes):

| wasmtime features | Clean build | Binary |
| --- | ---: | ---: |
| runtime only (cannot compile) | 20 s | 0.6 MB |
| runtime + Winch | 54 s | 7.5 MB |
| runtime + Cranelift | 59 s | 8.4 MB |

Cranelift itself adds about 1 MB and 5 s over Winch; the rest is wasmtime's compile machinery
(wasm translation, object output). Cranelift's optimization level (`none`, `speed_and_size`,
`speed`) changes neither run time nor compile time here. Compile time grows with module size, at
about 0.04 ms per IR instruction: 1.6 ms for a typical 20–40 instruction kernel, 25 ms for tap
delay 16, 80 ms for the splice reel. Instantiating a module is cheap (about 0.15 ms).

Earlier experiments in this branch tried Winch (wasmtime's baseline compiler, about 3× faster
to compile but no faster than the interpreter at run time) and running host calls from the loop
for built-in opcodes (about 2× slower than the interpreter). Both were removed.

### wasmi (a wasm interpreter) as the native runner

`--features niminal-engine/wasmi` runs the same modules in wasmi 2.0 (`custom/wasmi_run.rs`; eager
translation; wasmtime is still compiled in, so ignore its size). Audio is identical for all 16
workloads. Same benchmark and machine, median of three rounds, ns/frame:

| Workload | Our interpreter | wasmi | wasmtime |
| --- | ---: | ---: | ---: |
| scalar cascade | 164 | 68 | 24 |
| contour | 68 | 93 | 35 |
| filters | 82–109 | 116–183 | 43–61 |
| reverse | 132 | 370 | 43 |
| tap delay 4 / 16 | 728 / 1,956 | 1,904 / 5,188 | 300 / 751 |
| splice reel | 2,342 | 4,584 | 718 |

wasmi is 1.4–2.8× slower than our own interpreter on all but the synthetic cascade (2.4× faster:
long straight-line arithmetic). It does not replace the JIT.

### Keeping temporaries on the operand stack

A register written once and read once later (and consumed in stack order, with no block boundary
between) stays on wasm's operand stack instead of a local (`plan_stack` in `custom/wasm.rs`;
`NIMINAL_WASM_STACK=0` turns it off; `NIMINAL_WASM_STRATEGY=winch` selects Winch). In tap delay 128
of 473 results move to the stack. Three alternating rounds, ns/frame, audio identical:

| Workload | Interpreter | Cranelift, locals | Cranelift, stack | Winch, locals | Winch, stack |
| --- | ---: | ---: | ---: | ---: | ---: |
| contour | 76 | 34 | 31 | 64 | 62 |
| filter modulated | 110 | 60 | 61 | 134 | 125 |
| reverse | 137 | 42 | 41 | 150 | 139 |
| tap delay 16 | 2,011 | 733 | 676 | 2,493 | 2,273 |
| splice reel | 2,467 | 703 | 656 | 2,574 | 2,308 |
| scalar cascade | 168 | 23 | 23 | 88 | 40 |

Cranelift gains 0–8%. Winch gains 4–55% but is still no faster than the interpreter on anything
except the cascade.

### A hand-written aarch64 code generator (dynasm), spike

The native code generator (`NIMINAL_JIT=1`, the default) generates machine code
straight from the register IR with `dynasm`, no wasm in between (`custom/jit.rs`). It has a
linear-scan register allocator over live ranges (temporaries reuse FP registers; state and
loop-carried registers stay resident for the block), hoists constants out of the loop, writes
fused operations straight into their target register, and calls the same Rust helpers for `sin`,
`pow` and the like. It handles the scalar instructions, `Memo`, and state-list and table reads and
writes (`Read`, `ReadAt` with linear and cubic interpolation, `Write`), working directly on the
voice's own lists with no copying. Kernels that call a built-in opcode stay interpreted.

Three alternating rounds, ns/frame, audio identical to the interpreter's:

| Workload | Interpreter | dynasm | wasmtime (Cranelift) |
| --- | ---: | ---: | ---: |
| contour | 69 | 21 | 35 |
| slew | 52 | 14 | 19 |
| filter steady | 79 | 34 | 43 |
| filter modulated | 110 | 42 | 61 |
| filter stepped | 89 | 38 | 49 |
| scalar cascade | 165 | 22 | 24 |
| reverse | 131 | 22 | 43 |
| tap delay 4 | 716 | 135 | 300 |
| tap delay 16 | 1,956 | 303 | 751 |
| splice reel | 2,339 | 663 | 718 |

Compile time per kernel is 0.01–0.03 ms (wasmtime: about 1.6 ms for the same kernels, more for
large ones), and the binary grows by about 110 KB (wasmtime: about 8 MB stripped). Checked with
the same 400-kernel random differential test, an exhaustive operation-by-edge-value test
(`every_operation_matches_on_edge_values_on_the_jit`) and `cargo clippy`.

Not done: a Windows/Linux build, calling it from the audio thread safely under
concurrent recompilation, executable-memory permissions for signed builds.

### A piece that leans on it

`examples/kernels.nml` builds every sound from hand-written stateful `fn`s (phase accumulators, a
state-variable filter, Karplus-Strong, a chorus and a Schroeder room made of delay lines, a
congruential noise source, a clipper, a bit crusher, an envelope follower), so nearly all of its
per-sample work is kernels. The code generator takes 15 of the 16 kernels (`NIMINAL_JIT_VERBOSE=1`
lists them). Rendering 32 bars takes 13.9 s on the interpreter and 6.1 s with the JIT (2.3x), and the
two WAV files are byte-identical.

### x86-64 backend

`custom/jit/x64.rs` generates the same kernels for x86-64 (SSE2 plus SSE4.1; a CPU without SSE4.1
keeps the interpreter), always with the System V calling convention so Linux, macOS and Windows
share one code path. The allocator has 10 registers (`xmm6`-`xmm15`; `xmm0`-`xmm5` are scratch).
`jit/mod.rs` holds what both backends share: the live-range allocator, constant hoisting and the
kernel and voice types.

Only checked under Rosetta on an Apple Silicon Mac (`RUSTFLAGS="" cargo test --target
x86_64-apple-darwin ...`; the repository's `target-cpu=native` does not apply to another
architecture): the 400-kernel fuzz test, the edge-value test, and the engine, language and server
test suites (362 language tests) pass, and all 16 benchmark workloads render identical audio.
Rosetta translates both the interpreter and the generated code, so its timings (2.3-9.5x over the
interpreter on the covered kernels, but 1.4x on the cascade and slower on kernels that never reach
the JIT) say nothing reliable about real hardware. Not tested: real x86-64 hardware, Windows, Linux.

## What this shows, and what it does not

- One emitter gives 1.8–3.6× on pure-`.nml` kernels, including delay lines and the reel, where the
  earlier direct-Cranelift prototype gave 1.2–1.7×. Keeping registers in locals instead of memory
  is most of the difference.
- The emitted bytes are bit-identical to the interpreter in wasmtime and in V8, so sample-accurate
  stateful DSP carries over. The one cross-platform gap is `sin`, `exp`, `tanh`, `tan`, `log` and
  `pow`: they use each platform's maths library, so the last bits can differ between native and
  browser. Using the `libm` crate everywhere (interpreter, natives, imports) would close it.
- It does not help kernels that call built-in opcodes, which is most of the current shelf. Those
  need the opcodes themselves to run in the same module (compiled to wasm); not attempted.
- Cranelift compile time (tens of milliseconds, 150 ms for the reel) is a live-edit latency cost.
  A shipping design should keep the interpreter until the module is ready, then switch at a block
  boundary. The state layout (registers, state, lists) is shared, so the switch is exact. That
  switch is not built. In a browser the engine tiers up by itself.

## Prototype limits

- State lists and shared tables are copied into each voice's wasm memory (3 MB for the reel's
  tables, per voice). A real design needs tables in one memory shared by all voices of a kernel.
- Voice creation compiles and instantiates; it must happen off the audio thread. The server's voice
  path has not been checked for that.
- Compiled code is cached by kernel identity in a global list; there is no eviction beyond the
  kernel being dropped.
- Not run: x86-64, a signed Stage bundle (wasmtime's JIT memory), a wasm32 build of the engine, the
  playground itself, or concurrent compile-and-playback load.
- `scripts/check.sh --test` was run for the default build only. The feature is covered by its own
  tests (`cargo test -p niminal-engine --features wasm-kernel`).
