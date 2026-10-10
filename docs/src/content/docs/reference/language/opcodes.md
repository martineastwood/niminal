---
title: Opcodes and functions
description: User-defined opcodes, fn, state, scheduling, and Rust plugins.
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

- The **last expression** is the output.
- **`state`** declares a variable that persists between samples, per call site and per voice.
- **Multiple outputs** are named and returned as fields:

```nml
opcode svf(x, cutoff: hz, res: 0..1 = 0.5) -> (low, band, high) { ... }

f = noise().svf(cutoff: 2khz)
out = f.band
```

Opcodes can call opcodes, take trigger inputs, and declare latency (for compensation). **Rust plugins** implement the same opcode trait — built-in, niminal, and plugin opcodes look identical at the call site.

## Functions

`fn` runs at compile or load time: structure, helpers, score material. No audio state.

```nml
fn fifths(root: note, count: n) {
  for i in 0..count { root + (7st * i) }
}
```

Anonymous functions: `fn(a, b) { a * b }`. **`it`** covers the one-argument case.

## Scheduling from instruments

An instrument can schedule other notes with `schedule`:

```nml
instr echo_note(freq: hz, depth: n = 4) {
  out = pluck(freq: freq)
  if depth > 0 {
    schedule(echo_note(freq: freq * 3/2, depth: depth - 1), in: 1/8 beat)
  }
}
```

Scheduled notes go through the score model — logged, replayable, quantized like any other event.
