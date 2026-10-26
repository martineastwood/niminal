---
title: Syntax
description: Comments, blocks, bindings, calls and control flow in the orchestra language.
---

**Read by both programs.** The syntax is the same in the server's half and in niminal-live's.

## Comments and blocks

- **Comments:** `//` to end of line. A `///` comment above a definition is its documentation.
- **Blocks:** `{` `}`. Indentation is never significant, so evaluating a selected snippet is always safe.
- **Line continuation:** a line starting with `.` continues the previous expression.

## Bindings and names

- **Bindings:** no keyword. `name = expr` creates or rebinds a local; reads see the most recent binding above them.
- **Destructuring:** `[l, r] = src`.
- **Rebinding is not feedback:** `sig = sig.lpf(...)` rebinds the name. Feedback is always explicit with `prev(sig)`.
- **Unbound names** are compile errors.
- **No shadowing globals:** a local cannot share a name with a bus, instrument, fx, clip, sample, function, constant or other top-level declaration. Inside a function only the names it could read count: buses, controls, functions and constants.
- **Blocks:** a block can read and change every name above it; a name first bound inside a block stays inside it.
- **Private names:** top-level names starting with `_` are private to their module.

## Operators

From the tightest to the loosest:

| Operators | Notes |
| --- | --- |
| `f(...)`, `x.f`, `x[i]` | Calls, methods, indexing |
| `-x`, `not x` | |
| `*`, `/` | Dividing by zero gives 0 |
| `+`, `-` | |
| `..`, `..<` | Ranges: `1..8` includes 8, `0..<8` stops before it |
| `<`, `>`, `<=`, `>=`, `==`, `!=` | Give 1 or 0; they don't chain |
| `and` | 1 when both are non-zero |
| `or` | 1 when either is non-zero |

- **The remainder** is `x.mod(y)`, with the sign of `y`. `%` only means percent.
- **No power operator:** `pow(x, y)`.
- **Math** works everywhere: `sin cos tan tanh exp log sqrt abs sign floor ceil fract`, `min(a, b)`, `max(a, b)`, `pow(x, y)`, `x.clamp(lo..hi)`; constants `pi`, `sample_rate`, `true`, `false`.

## Calls and chaining

- **Named arguments** by default. A callee's leading required parameters may be given in order: `osc(saw, freq)`, `sig.lpf(800hz)`.
- **UFCS:** `x.f(a)` is `f(x, a)`. The dot is left-to-right wiring.
- **Paren-less calls only after a dot:** `riff.reverse`, `sig.tanh`. A bare name is a reference, never a call.
- **After a dot**, a result field (`f.band`) comes first, then a function. Ambiguity is an error.
- **Arrays expand built-ins element by element:** `osc(saw, [f, f * 1.01])` is an array of two oscillators. It becomes audio channels at an output or layout conversion; `.sum` adds its elements. Array lengths are independent of the audio channel limit.

## Buses and `out`

- **Buses** are written only with `+=`, from instruments and fx declarations (not functions).
- **`out`** is the one name that also allows `=`.

## Implicit names

**`out`**, **`in`**, **`it`** and **`note`**:

- **`out`:** the result of an instrument or function (see [Instruments](/reference/language/instruments/)).
- **`in`:** the audio coming into an `fx`. It pairs with `out`: `out = in.lpf(cutoff: 1khz)`.
- **`it`:** the current value in a one-argument context: a function given to `map`.
- **`note`:** the running voice in an instrument (`note.time`, `note.released`), or the note being tested in a pattern's `where(note < c3, transpose(+12st))`.

## Control flow

- **`if` / `else`** is an expression or a statement. Each branch is a block:

```nml
gain = if freq < 2khz { 0db } else if freq < 6khz { -6db } else { -14db }

if trig > 0 and prev(trig) <= 0 { held = x }
```

- **`for` loops run while compiling**, building graph structure. There are no unbounded loops on the audio thread.

```nml
instr organ(freq: hz) {
  for n in 1..8 {
    out += osc(sine, freq * n).gain(-3db * (n - 1))
  }
}

clipials = for n in 1..8 { osc(sine, freq * n) / n }
```
