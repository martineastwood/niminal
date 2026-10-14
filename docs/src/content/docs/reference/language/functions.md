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
- **`prev(x)`** is `x` one sample ago. `y = x + prev(y) * 0.5` is a feedback loop, written on one line. (In a bus send, `prev` means a block late instead; see [Routing](/reference/language/routing/).)
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

## Decisions and loops

- **`if`** as a value needs an `else`; each branch is a block ending with its value. As a statement it can change names, state and `out`:

```nml
fn hold(x, trig) {
  state h = 0
  if trig > 0 and prev(trig) <= 0 { h = x }   // sample and hold, on a rising edge
  h
}
```

  A condition known while compiling picks its branch then; otherwise both are worked out and the result is chosen each sample, and state is only written on the branch taken.
- **`for x in xs { … }`** repeats a block a number of times known while compiling: over a range or an array, including a state buffer. As a value it gives an array. As a statement, assigning to a name from outside carries the value from one pass to the next.

## Arrays

Arrays are the general collection type. Tables and writable DSP buffers use the same array operations. A signal is an array of channels with a rate, a unit and a channel layout.

- `[a, b, c]`, `for` and `.map(…)` make fixed-length arrays. Elements can be constants, changing values or nested arrays. `[num]`, `[hz]` and `[[num]]` check element types in function parameters. Functions with state can accept and return computed arrays of any fixed length.
- Lengths are known while compiling. Arrays do not inherit the audio channel limit: a 2048-element table or computed frame is an array. Audio output and layout conversion enforce the channel limit.
- Arithmetic, comparisons, math, unit conversion, `.range` and `.clamp` work element by element. A scalar broadcasts to all elements; two arrays must have matching shapes. Built-in DSP operations expand over arrays and return arrays, so `osc(saw, frequencies)` can make a large oscillator bank.
- `xs[i]` floors a fractional index and wraps (`xs[-1]` is the last). A changing index reads an element each sample; the possible elements must have matching shapes and units. Slices such as `xs[0..<4]` produce arrays of the selected values.
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

`const` numeric arrays are worked out while compiling and their storage is shared by voices. They can be mapped, indexed, interpolated and reduced just like other arrays:

```nml
const saw8 = harmonics([1, -1/2, 1/3, -1/4], size: 4096)
const taps = [0.25, 0.5, 0.25]
```

## Built-ins

Core opcodes (`osc`, filters, `gain`, `delay`, `reverb`, `pan`, math, …) are listed under [Built-in opcodes](/reference/opcodes/). That page is generated from the compiler, so it stays in sync with the binary. A function can call any of them except `pan` and `to_layout` inside a function with state. Other DSP built-ins expand element by element over array arguments there as well.
