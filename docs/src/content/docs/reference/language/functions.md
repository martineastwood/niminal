---
title: Functions
description: fn, state, lists, rates and chaining.
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
| `taps: [num]` | A list. A multichannel signal passed here is a list of its channels |
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

- **The last expression** is the result, or build **`out`** with `out = …` and `out += …` (not both).
- **`-> hz`** declares the result's unit, which is checked.
- **Several results** are named after `->`; the body binds each, and they come back as fields. The body runs once for all of them.

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
- **A state list** holds a buffer: `state buf = zeros(4096)`, written with `buf[i] = y` and read with `buf[i]` or `buf.at(pos, interp: linear)`. Its length is known while compiling. Indexes wrap, so a read or write can never fail.

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
- **`for x in xs { … }`** repeats a block a number of times known while compiling: over a range, a list or a table. As a value it gives a list. As a statement, assigning to a name from outside carries the value from one pass to the next.

## Lists and tables

- `[a, b, c]`, ranges (`0..<8`), `for` and `.map(…)` make lists. They exist while compiling; each entry can be anything.
- `xs[i]` counts from 0 and wraps (`xs[-1]` is the last). With an index that changes, it switches between the entries each sample.
- `xs.len`, `xs.sum`, `xs.min`, `xs.max`, `xs.indices`, `xs.map(f)`, slices `xs[0..<4]`.
- `.map` takes `fn(x) { … }`, or an expression of `it`: `(1..8).map(1 / it)`.
- **Tables** are lists of numbers defined with `const`, worked out once and shared by every voice:

```nml
const saw8 = harmonics([1, -1/2, 1/3, -1/4], size: 4096)
const taps = [0.25, 0.5, 0.25]
```

  `table.at(pos, interp: linear)` reads between entries (`none`, `linear` or `cubic`).

## Built-ins

Core opcodes (`osc`, filters, `gain`, `delay`, `reverb`, `pan`, math, …) are listed under [Built-in opcodes](/reference/opcodes/). That page is generated from the compiler, so it stays in sync with the binary. A function can call any of them except `pan` and `to_layout` inside a function with state, which works on one channel at a time.
