---
title: Functions
description: fn, state, arrays, rates and chaining.
---

`fn` is the one way to write reusable code. What a function becomes depends on its body:

- **Without `state`** it is pure, and works at any rate: while compiling (a constant, a table), at note start, per block or per sample.
- **With `state`** (a `state` line, `prev`, or a call to a function that has one) it is a unit generator. Each call has its own state, for each voice, and runs sample by sample.

```nml
/// A one-pole low-pass filter.
fn one_pole(x, cutoff: hz) {
  state y = 0
  a = exp(-2 * pi * cutoff / sample_rate)   // worked out only as often as cutoff changes
  y = x * (1 - a) + y * a
  y
}

fn mtof(n: num) -> hz { 440hz * pow(2, (n - 69) / 12) }

out = osc(saw, mtof(48)).one_pole(800hz)
```

- **Definition order doesn't matter.** A function can call one defined later; calling itself, directly or through others, is an error.
- **No side effects:** a function can't send to a bus, read `note` or `input`, or send OSC. Its instrument does those.
- **Doc comments:** a `//` or `///` comment directly above a `fn` or `instr` is its hover documentation in the editor.

## Parameters

```nml
fn f(x, cutoff: hz, res: 0..1 = 0.5, freq: init hz, size: const int = 4096, mode: (low | band | high) = low, taps: [num])
```

| Written | Accepts |
| --- | --- |
| `x` | Anything: the unit comes from each call (`fn twice(x) { x * 2 }` works on hz, db or plain numbers) |
| `cutoff: hz` | A value in hz. The types are `num`, `int`, `hz`, `db`, `sec`, `st` and `deg` |
| `res: 0..1`, `cut: 20hz..20khz` | A value between the ends, clamped to them |
| `mode: (low \| band \| high)` | One of the names, known while compiling. Inside the body, `mode == low` picks a branch then |
| `make: fn` | A user function name or `fn(x) { … }`, known while compiling; anonymous functions capture values from where they are written |
| `taps: [num]` | A fixed-length array whose elements are checked as plain numbers. Signals supply their channels, including a mono signal as one element |
| `= default` | Makes the parameter optional. A default may use earlier parameters |

A **rate** before the type says what the function needs from the caller:

| Rate | Inside the body the value is | When the caller passes something faster |
| --- | --- | --- |
| *(none)* | Whatever was passed | Nothing changes |
| `control` | At most once per block | Read at the start of each block |
| `init` | Fixed at note start | Its value at note start is used |
| `const` | Known while compiling | Compile error |

**Arguments:** the leading required parameters may be given in order; the rest are named. `sig.lpf(800hz)` and `osc(saw, 440hz)` work, `sig.lpf(800hz, 0.3)` doesn't (write `res: 0.3`).

## Functions as parameters

A `fn` parameter takes a named user function or an anonymous function. Calls are specialized while compiling; there is no runtime dispatch. A function value can be forwarded to another function or supplied to `.map`. Defaults can name another function.

```nml
fn unison(freq: hz, make: fn, voices: const int = 7) {
  layers = for i in 0..<voices {
    make(freq * (1 + (i - (voices - 1) / 2) * 0.003))
  }
  layers.sum / voices
}

instr pad(freq: hz, cutoff: hz = 2khz) {
  unison(freq, fn(f) { osc(saw, f).lpf(cutoff) }) * adsr(10ms, 100ms, 0.6, 300ms)
}
```

Anonymous functions capture the surrounding values they read. Each invocation has independent state, including when expanded by a loop. A callback's argument count, units and result are checked when its caller is specialized. Bodies of functions with `fn` parameters are checked at their calls, rather than against a guessed callback at definition time. Function parameters cannot have `init` or `control` rates, and instruments can accept them as structural parameters with defaults.

## Results

- **The last expression** is the result, including an array or a nested array, or build **`out`** with `out = …` and `out += …` (not both).
- **`-> hz`** declares the result's unit, which is checked.
- **Several results**, each of which can also be an array, are named after `->`; the body binds each, and they come back as fields. The body runs once for all of them.

```nml
fn svf(x, cutoff: hz, res: 0..1 = 0.5) -> (low, band, high) {
  state low = 0
  state band = 0
  f = 2 * sin(pi * cutoff / sample_rate)
  high = x - low - 2 * (1 - res) * band
  band = band + f * high
  low = low + f * band
}

f = osc(saw, 110hz).svf(cutoff: 2khz)
out = f.band
```

Used where one value is expected, a function with several results gives the first.

## State

- **`state y = 0`** keeps `y` from one sample to the next. A read above an assignment sees last sample's value; a read below it sees the new one.
- **The starting value** is worked out at note start and can use the parameters: `state y = x` starts at the input's first sample. Its unit is the state's unit, so `state y = 0` holds plain numbers and `state y = 0hz` frequencies.
- **`prev(x)`** is `x` one sample ago. `y = x + prev(y) * 0.5` is a feedback loop, written on one line. (In a bus send, `prev` is an error: `late(x)` is the block-late copy; see [Routing](/reference/language/routing/).)
- **A state array** holds a buffer: `state buf = zeros(4096)`, written with `buf[i] = y` and read with `buf[i]` or `buf.at(pos, interp: linear)`. Its length is known while compiling. Indexes wrap in both directions; reading or writing an empty array is a compile error. It supports the same mapping, arithmetic, slices and reductions as other arrays. Its initial elements can use note-start parameters.

```nml
/// A plucked string: Karplus-Strong.
fn string(x, freq: init hz, decay: 0..1 = 0.996, size: const int = 4096) {
  state buf = zeros(size)
  state pos = 0
  len = sample_rate / freq
  delayed = buf.at(pos - len, interp: linear)
  y = x + (delayed + prev(delayed)) * 0.5 * decay
  buf[pos] = y
  pos = (pos + 1).mod(size)
  y
}
```

## Reset scopes

`reset_on(trigger) { … }` evaluates a DSP block and restarts state created inside its body on every positive edge: when the trigger changes from zero or negative to positive. An initially positive trigger resets on the first sample. Holding it positive does not repeatedly reset it.

```nml
instr pulse(freq: hz) {
  trigger = osc(square, 4hz) * (1 - note.released)
  reset_on(trigger) {
    osc(saw, freq) * env[1 150ms.exp 0]
  }
}
```

Resets happen before processing that sample, including between block boundaries. They restart nested functions, built-in DSP effects, `prev`, state arrays, and initialization inside the scope. State initializers read the current inputs again. Scopes may nest; resetting an outer scope also restarts the inner one. Resetting clears existing delay and reverb contents.

Signals calculated outside the scope keep running. Pass audio into a scope to reset only its processing, or create the generator inside to restart both. A scope returns its last expression or its own `out`, and its locals stay inside. It may declare `state` at the beginning, just like a function. Stateful scope bodies follow the function rules: bind note context outside and pass it in through a captured value. All storage is allocated before rendering; resetting does not allocate.

## Custom completion

`signal.finish_when(done)` silences a layer permanently when `done` becomes positive and reports that layer as finished. It works inside custom functions as well as instruments. A reset scope can restart it.

```nml
fn decay(released, amount: 0..1 = 0.9995) {
  state level = 1
  if released { level = level * amount }
  level.finish_when(level < 0.00001)
}

instr tone(freq: hz) {
  osc(sine, freq) * decay(note.released)
}
```

Completion follows the existing envelope rules: the voice waits until release, all envelopes and custom completion signals have finished, and downstream delay/reverb tails have drained. `tail:` still applies. Each layer can report completion independently. Choose a completion point where the layer is already silent to avoid an abrupt cut.

## Decisions and loops

- **`if`** as a value needs an `else`; each branch is a block ending with its value. As a statement it can change names, state and `out`:

```nml
fn latch(x, trig) {
  state h = 0
  if trig > 0 and prev(trig) <= 0 { h = x }   // sample and hold, on a rising edge
  h
}
```

  A condition known while compiling picks its branch then; otherwise both are worked out and the result is chosen each sample, and state is only written on the branch taken.
- **`match name { … }`** is the `else if` chain on one name, for choices or numbers. An arm is one or more patterns joined by `|`, a colon, and a value or a block; `else:` takes the rest. As a value it needs an `else`.

```nml
fn pick(f, mode: (low | band | high) = low) {
  match mode {
    low: f.lpf(cutoff: 800hz)
    band | high: f.hpf(cutoff: 800hz)
  }
}
```

- **`for x in xs { … }`** repeats a block a number of times known while compiling: over a range or an array, including a state buffer. As a value it gives an array. As a statement, assigning to a name from outside carries the value from one pass to the next.

## Arrays

Arrays are the general collection type. Tables and writable DSP buffers use the same array operations. A signal is an array of channels with a rate, a unit and a channel layout.

- `[a, b, c]`, `for` and `.map(…)` make fixed-length arrays. Elements can be constants, changing values or nested arrays. `[num]`, `[hz]` and `[[num]]` check element types in function parameters. Functions with state can accept and return computed arrays of any fixed length.
- Lengths are known while compiling. Arrays do not inherit the audio channel limit: a 2048-element table or computed frame is an array. Audio output and layout conversion enforce the channel limit.
- Arithmetic, comparisons, math, unit conversion, `.range` and `.clamp` work element by element. A scalar broadcasts to all elements; two arrays must have matching shapes. Built-in DSP operations expand over arrays and return arrays, so `osc(saw, frequencies)` can make a large oscillator bank.
- `xs[i]` floors a fractional index and wraps (`xs[-1]` is the last). A changing index reads an element each sample; the possible elements must have matching shapes and units. Slices such as `xs[0..<4]` produce arrays of the selected values.
- `(0..1).step(0.25)` is the array `0, 0.25, 0.5, 0.75, 1` for a range that isn't whole numbers (`(0..<1).step(0.25)` leaves the end out); the step is in the range's unit.
- `xs.active(count)` keeps the first `count` entries and silences the rest, where `count` may change while playing: a bank of 16 partials of which a parameter chooses how many sound, without recompiling.
- `xs.len`, `xs.sum`, `xs.min`, `xs.max`, `xs.indices`, `xs.map(f)` work on constants, computed arrays and state buffers. Reducing nested arrays combines their rows element by element. `.map` takes `fn(x) { … }`, or an expression of `it`.
- `xs.at(pos, interp: linear)` reads between numeric elements with wrapping. Interpolation is `none`, `linear` (the default) or `cubic`, for every storage type.
- `[]`, `zeros(0)` and empty slices are valid arrays. They support length, mapping and iteration. Indexing, interpolation and reductions on an empty array are compile errors.
- Signals offer these operations directly: `signal[0]`, `signal.map(it * 0.5)` and `signal.sum`. Mapping and element-wise processing preserve a signal's channel layout. `.channels` explicitly gives its channels as an ordinary array.
- Ordinary array values are snapshots. Mapping, slicing, passing or returning a state buffer reads its current elements. Only a buffer declared with `state` can be written using `buffer[i] = value`. Each call and voice has its own buffer; processing does not resize arrays or allocate storage.

```nml
fn smooth_bank(xs: [num], amount: 0..1 = 0.04) {
  state history = zeros(xs.len)
  for i in xs.indices {
    history[i] = history[i] + (xs[i] - history[i]) * amount
  }
  history
}

instr cloud(freq: hz) {
  frequencies = (0..<32).map(freq * (1 + (it - 15.5) * 0.001))
  osc(saw, frequencies).smooth_bank(amount: 0.04).sum / 32
}
```

### Constant tables

`const` numeric arrays are worked out while compiling and their storage is shared by voices. Named constructors (`harmonics`, `curve`, `cheby`, `window`, `file`, …), transforms and readers are documented under [Tables](/reference/language/tables/).

```nml
const saw8 = harmonics([1, -1/2, 1/3, -1/4], size: 4096)
const taps = [0.25, 0.5, 0.25]
```

An oscillator plays a table as one cycle of a wave, band-limited at any pitch: `osc(saw8, freq)`.

## Vector operations

Arrays held in memory (a state array, a table, or the result of one of these operations) are worked on a whole array at a time, so a line costs about the same however long the array is. Operations that involve a state array or one of these results:

- **Arithmetic and math:** `+ - * /`, `min`, `max`, `-x` and the one-argument math functions, entry by entry. A single value is used for every entry, and two arrays need the same length: `y = x * 0.01 + 1.98 * cos(w) * s1 - 0.9801 * s2`.
- **`buf = expr`** writes every entry of a state array you own, from an array of the same length (or one value for all): `s2 = s1` followed by `s1 = y` is one step of a delay per entry.
- **`xs.sum`, `xs.min`, `xs.max`** reduce an array to one value. A sum is added in four parts, in a fixed order, so it is the same on every machine (but not bit-for-bit what a left-to-right loop gives).
- **`a.dot(b)`** is `Σ a[i] * b[i]`, and **`m.matvec(v)`** multiplies a matrix stored row by row (a table or array with `rows × cols` entries) by a vector of `cols` entries, giving `rows` entries.
- **`buf.fir(taps, pos)`** is `Σ taps[i] * buf[pos - i]` over a state array used as a ring, newest first, wrapping: a filter output without a loop. `taps` is no longer than the ring.
- **`buf.taps(offsets, pos, interp: linear)`** reads the ring at many offsets back from `pos` (each can be fractional and can change every sample) and gives an array of the values: a multitap delay.

```nml
/// Eight resonators in parallel: modal synthesis of a bar or a drum.
const modes = [1, 2.76, 5.4, 8.9, 13.3, 18.6, 24.8, 31.9]
fn chime(x, freq: init hz) {
  state s1 = zeros(8)
  state s2 = zeros(8)
  w = 2 * pi * freq / sample_rate * modes     // worked out once, at note start
  y = x * 0.01 + 1.98 * cos(w) * s1 - 0.9801 * s2
  s2 = s1
  s1 = y
  y.sum
}
```

Whole pieces that use these forms, with audio, are on the [standard library](/reference/language/standard-library/#examples) page (`examples/modal.nml` and `examples/multitap.nml`).

`init` on `freq` lets everything that depends only on it be worked out when the note starts. These forms need a function with state; using one makes a function a kernel. These names, like the other built-ins, can't be used for a function you write: see [Reserved words](/reference/reserved-words/).

## Frames and the FFT

`x.frames(size, fn(w) { ... })` runs its function every `size / 2` samples on the last `size` samples of `x`, windowed with a Hann window, as an array `w`. The function returns `size` samples, which are overlapped into the output. If it returns `w`, the output is `x` exactly, `size` samples late. `size` is a power of two from 4 to 65536.

- **`fft(w)`** gives `[re, im]`, the spectrum of an array of `n` samples, each with `n / 2 + 1` entries. **`ifft(re, im)`** is its inverse (also `ifft([re, im])`). `atan2(im, re)` gives a phase and `sqrt(re * re + im * im)` a magnitude. `fft` works anywhere in a function with state; in a frame it runs once per half frame.
- **State** declared at the top of the function lasts from frame to frame, so a spectrum can be held, smeared or accumulated.
- **Latency** is `size` samples, declared to the graph, so a path beside the function is delayed to match.
- **Size is the user's**, not the engine block's, so the sound doesn't depend on `--block-size` and a log replays exactly.

```nml
/// Keep only the lowest 40 bins: a brick-wall low-pass.
const keep = (0..<513).map(it < 40)
fn low(x) {
  x.frames(1024, fn(w) {
    s = fft(w)
    ifft(s[0] * keep, s[1] * keep)
  })
}
```

`examples/spectral.nml` is a piece built from `frames`, `fft` and `ifft`.

## Tables as arguments

A table can be an argument: a parameter written `curve: [num]` takes one, and so does an opcode's table parameter. `x.waveshape(table)` maps `x` in -1..1 through a transfer table; `osc(table, freq)` plays one as a wave, and several tables (an array of them) as a bank to morph with `position:`; `x.convolve(ir)` convolves with an impulse response (see below). `waveshape` and `osc` work inside functions with state; `convolve` works on whole blocks, so it goes in a function without state, or outside it.

```nml
fn shape(x, curve: [num]) { x.waveshape(curve) }
out = shape(osc(sine, 110hz), cheby([1, 0.3]))
```

## Built-ins

Core opcodes (`osc`, filters, `gain`, `delay`, `reverb`, `pan`, math, …) are listed under [Built-in opcodes](/reference/opcodes/). That page is generated from the compiler, so it stays in sync with the binary. A function can call any of them except `pan` and `to_layout` inside a function with state. Other DSP built-ins expand element by element over array arguments there as well.

## Oversampling

`oversample(2) { ... }`, `oversample(4) { ... }` and `oversample(8) { ... }` run sample DSP at a higher rate with filtered upsampling and downsampling:

```nml
instr driven(freq: hz) {
  oversample(4) {
    (osc(saw, freq) * 5).tanh
  }
}
```

Create generators and nonlinear DSP effects inside the scope to run them at its sample rate. A signal calculated outside is upsampled; its generator still runs at the original rate. `sample_rate` inside is the higher rate, keeping oscillators, delays, filters and user-written DSP at the intended pitch and time scale. Input conversion preserves constant values. The conversion filters add 16 samples at the enclosing scope’s rate. The compiler aligns parallel paths automatically, including across instruments, nodes and feed-forward buses. See [Latency compensation](/reference/language/routing/#latency-compensation).

The body supports state, arrays, envelopes and DSP functions. It follows the sample DSP function restrictions on channel-layout processing and side effects. Scopes can nest up to 16 levels and return arrays or named results. An oversampled spawned voice keeps independent state and release behavior. Capture transport or other parent context in a local before entering the scope. Read slow controls with `.control` outside the scope when they must retain the parent's block cadence; processing inside is sample by sample.

```nml
instr cloud(freq: hz) {
  spawn(osc(square, 20hz), dur: 80ms, max: 8) {
    oversample(4) {
      (osc(saw, freq) * 3).tanh * env[0 2ms 0.2 | 20ms 0]
    }
  }
}
```

A node can use `oversample(4) { in.tanh }` directly. Each nested scope uses the sample rate of its immediate parent; two nested 2x scopes run the inner DSP at 4x the project rate. `spawn` captures the values it reads when a layer starts; placing an effect inside that layer gives in independent state.
