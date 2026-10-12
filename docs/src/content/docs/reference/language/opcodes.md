---
title: Opcodes
description: User-defined opcodes, state, and chaining.
---

## User-defined opcodes

`opcode` defines a unit generator: it runs in the audio graph, can hold per-voice state between samples, and chains like any built-in.

```nml
opcode one_pole(x, cutoff: hz) {
  state y = 0.0
  a = exp(-2 * pi * cutoff / sample_rate)
  y = x * (1 - a) + y * a
  y
}

opcode drive(x, amount: 0..1 = 0.5) {
  (x * (1 + amount * 9)).tanh
}

out = osc(saw, freq).drive(amount: 0.3).one_pole(cutoff: 800hz)
```

- The **last expression** is the output (or use `out` the same way as in an instrument).
- **`prev(x)`** is `x` one sample ago, so feedback is explicit: `y = x + prev(y) * 0.5` reads `y` before it is defined. `prev` of a name that is never defined is an error. (Between buses, `prev` works differently: it sends a block late. See [Routing](/reference/language/routing/).)
- **`state`** declares a variable that persists between samples, per call site and per voice.
- Inside opcodes, **`pi`** and **`sample_rate`** are available.
- **Multiple outputs** are named and returned as fields:

```nml
opcode split(x, cutoff: hz) -> (low, high) {
  low  = x.lpf(cutoff: cutoff)
  high = x.hpf(cutoff: cutoff)
}

bands = sig.split(cutoff: 1khz)
out = bands.low + bands.high.gain(-6db)
```

Opcodes can call other opcodes. A comment (`//` or `///`) directly above an `opcode` or `instr` is its hover documentation in the editor.

## Built-ins

Core opcodes (`osc`, filters, `gain`, `delay`, `reverb`, `pan`, math, …) are listed under [Built-in opcodes](/reference/opcodes/). That page is generated from the compiler, so it stays in sync with the binary.
