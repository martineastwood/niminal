---
title: Syntax
description: Comments, blocks, bindings, calls and control flow in the orchestra language.
---

## Comments and blocks

- **Comments:** `//` to end of line.
- **Blocks:** `{` `}` — indentation is never significant, so evaluating a selected snippet is always safe.

## Bindings and names

- **Bindings:** no keyword. `name = expr` creates or rebinds a local; reads see the most recent binding above them.
- **Destructuring:** `[l, r] = src`.
- **Rebinding is not feedback:** `sig = sig.lpf(...)` rebinds the name. Feedback is always explicit with `prev(sig)`.
- **Unbound names** are compile errors; bindings that are never read produce warnings.
- **No shadowing globals:** a local cannot share a name with a bus, instrument, track, sample, or other top-level declaration.
- **Private names:** top-level names starting with `_` are private to their module.

## Calls and chaining

- **Named arguments** by default. An obvious first argument may be positional: `osc(saw, freq)`.
- **UFCS:** `x.f(a)` is `f(x, a)`. Opcodes stay freestanding; the dot is left-to-right wiring.
- **Paren-less calls only after a dot:** `sig.ifft`, `riff.reverse`. A bare name is a reference, never a call.
- **After a dot**, resolution order is: field on the value’s type, then unit, then opcode. Ambiguity is an error.
- **Line continuation:** a line starting with `.` continues the previous expression.
- **No bit-shift operators:** use `shl(x, n)` and `shr(x, n)`.

## Buses and `out`

- **Buses** are written only with `+=`.
- **`out`** is the one name that also allows `=`.

## Implicit names

The language has exactly two implicit names: **`out`** and **`it`**.

- **`out`:** instrument, opcode, or `fn` result (see [Instruments](/reference/language/instruments/)).
- **`it`:** the current value in a one-argument context (e.g. `split`, anonymous `fn`, pattern transforms).

## Control flow

- **`if` / `else` is an expression:**

```nml
gain = if it.freq < 2khz { 0db } else { -14db }
```

- **`for` loops run at compile time**, building graph structure or score material. There are no unbounded loops on the audio thread.

```nml
instr organ(freq: hz) {
  for n in 1..8 {
    out += osc(sine, freq * n).gain(-3db * n)
  }
}
```
