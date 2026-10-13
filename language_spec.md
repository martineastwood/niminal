# niminal language: functions and instruments

Draft for review · Oct 13, 2026

This document defines the core of the niminal language: values, functions (`fn`) and instruments (`instr`, `mono`). It is the base that every opcode will be built on, so it comes before growing the opcode library. Once agreed, it replaces the "Orchestra" sections of `niminal Spec.md` that cover syntax, opcodes, functions and instruments. Tracks, clips, scenes, patterns and the score model are out of scope here; they keep their own sections and reuse the statement and expression rules below.

Markers show how each rule relates to the current implementation:

- **[built]** works today as described.
- **[change]** exists today but behaves differently; this document wins.
- **[new]** not built yet.

## 1. Principles

1. **One way to say a thing.** Each concept has one spelling. When two forms compete, one goes.
2. **Concise for the stage, readable at length.** Short where it is typed live (`sig.lpf(800hz)`), explicit where it is read later (named arguments, units).
3. **Inferred by default, declared at the boundaries.** Units, rates and shapes are inferred inside a body. Parameters say what a function needs.
4. **Nothing on the audio thread can fail.** No allocation, no unbounded loops, no out-of-bounds reads, no division errors.
5. **Braces, never indentation.** Any selected block can be evaluated on its own.
6. **Built-ins and user code look the same.** A built-in opcode is a `fn` written in Rust. Users can't tell the two apart when calling them.

## 2. Lexical structure

- **Comments:** `//` to the end of the line. **Doc comments:** `///` before a `fn`, `instr` or parameter. They are shown on hover and in completion. **[new]**
- **Statements** end at a newline. Newlines inside `( )` and `[ ]` are ignored. A line starting with `.` continues the previous expression. **[built]**
- **Blocks** are `{ ... }`. Indentation never matters. **[built]**
- **Names** are `snake_case`. A top-level name starting with `_` is private to its module. **[built]**
- **Numbers:** `0.5`, `440`, `-3`, and rationals with an attached unit (`1/8beat`). **[built]**
- **Units attach directly** to a literal (`440hz`, `-6db`, `5ms`). Outside brackets, a space is also allowed (`4 bars`). **[built]**
- **Strings:** `"drums/kick.wav"`. They are used only for paths and addresses. **[built]**
- **Brackets in a body** (`fn`, `instr`, track or bus) are lists. Patterns (`[c2 ~ eb2]`) belong to the performance layer. **[built]**

## 3. Values

Every value has three properties, all inferred:

| Property | Answers | Examples |
| --- | --- | --- |
| **Unit** | What it measures | `num`, `hz`, `db`, `sec`, `st`, `deg` |
| **Rate** | How often it changes | `const`, `init`, `control`, `audio` |
| **Shape** | How many of it there are | a single value, a list, a multichannel signal |

### 3.1 Units

The unit rules of `niminal Spec.md` stay as they are **[built]**. A short summary:

- Plain numbers (`num`) are unitless. Oscillators, noise, envelopes and LFOs produce `num`.
- `num × hz = hz`, `hz + hz = hz`, `hz / hz = num`. Adding values of different units is an error.
- Related units convert: note names to hz (`c4`), semitones applied to a frequency (`freq + 7st`), beats to seconds at the current tempo, and db as a gain.
- `x.num` strips a unit and `x.hz` (or `.db`, `.sec` and so on) attaches one to a plain value. **[new]**
- A bare number where a unit is expected is an error with a quick fix (`cutoff: 800` → "did you mean `800hz`?"). Gain parameters also accept a plain linear factor.
- **`int`** is a `num` that must be a whole number. It is used for sizes, counts and indices. **[new]**
- **`true` and `false`** are `1` and `0`. **[new]**

### 3.2 Rates

A rate says how often a value can change. There are four, from slowest to fastest:

| Rate | Csound | Changes | Typical uses |
| --- | --- | --- | --- |
| `const` | (orchestra load) | Never: known while compiling | List sizes, loop counts, a delay's `max`, FFT size, `mode:` choices, tables |
| `init` | i-rate | Once, at note start | A plucked string's length, a random seed per note |
| `control` | k-rate | Once per block (32 samples by default) | Slow modulation, filter coefficients when CPU matters |
| `audio` | a-rate | Every sample | Signals, audio-rate modulation |

**Inference.** A value's rate is the fastest rate among its inputs **[new]**:

- Literals, `pi`, `sample_rate` and tables are `const`.
- A note's parameters are `audio` unless declared otherwise (§5.2), because lanes, glides and MPE can move them.
- Oscillators, envelopes and state are `audio`.
- `freq * 2` has the rate of `freq`. `exp(-2 * pi * cutoff / sample_rate)` has the rate of `cutoff`.

**Hoisting.** Every expression is computed at its own rate. If `cutoff` only changes per note, the `exp(...)` above runs once per note, even inside a function that otherwise runs every sample. This is what Csound's i/k/a prefixes were for, and it needs no syntax. **[new]**

**Changing a rate explicitly.** A value never needs to be made faster: an `init` value feeds an audio input as is. To make a value slower, use a method:

| Method | Gives |
| --- | --- |
| `x.init` | The value `x` had at note start (Csound's `i(k)`) |
| `x.control` | `x` read once per block and held |

These are the only two rate methods. `.audio` is not needed. **[new]**

**Rate requirements are checked.** Passing a value that changes per note where `const` is needed is an error that names the cause, for example: "`size` must be known while compiling, but `len` is set at each note start". **[new]**

#### Why bindings carry no rate or type annotations

Writing `init x = 0.5` or `control y = 5hz` would add a third way to state a rate, after parameter types and the `.init` and `.control` methods. Most of the time it would only repeat what is already known: `0.5` and `5hz` are literals, so they are already `const`.

A local's rate is decided entirely by its inputs. The two reasons to change it are both covered by methods, which also work inline:

```
start = freq.init                       // freeze at note start
wobble = lfo(5hz).control               // cheaper: once per block
osc(sine, freq.init)                    // inline, no binding needed
```

Parameters are where annotations earn their place, because they are the contract with the caller (§5.2). If dogfooding shows that local annotations help readability, the parameter syntax can be allowed on bindings later (`cut: hz = ...`). Until then, bindings stay `name = expr`.

### 3.3 Shapes: values, lists and channels

- **A value** is one number or one signal.
- **A list** has a length known while compiling. Its entries can be anything, including signals and other lists (§7). **[built]**
- **A multichannel signal** is a list of signals that carries a channel layout (`stereo`, `surround(5.1)`, …). A list becomes channels where it reaches `out`, a bus, or a parameter that takes channels. **[built]**

## 4. Ranges

A range is a value with two ends, written with `..` **[change]**:

| Form | Meaning |
| --- | --- |
| `a..b` | From `a` to `b`, **including** `b` |
| `a..<b` | From `a` up to, not including, `b` **[new]** |

- **Ranges carry units:** `20hz..20khz`, `-12db..0db`, `0..1`. Both ends must have the same unit.
- **Ranges are values:** `sweep = 200hz..4khz` can be passed around and reused.
- **Integer ranges iterate:** `for n in 1..8` runs eight times and `for i in 0..<n` runs `n` times. `(0..1).step(0.25)` gives `0, 0.25, 0.5, 0.75, 1`.
- **One spelling everywhere.** Every function that needs two ends takes a range, never a `lo, hi` pair:

| Use | Example |
| --- | --- |
| Bounded parameter type | `bright: 0..1`, `cutoff: 20hz..20khz` |
| Loop | `for i in 0..<8 { ... }` |
| Slice | `buf[0..<16]` |
| Map a 0..1 signal | `lfo(2hz).range(200hz..4khz)` |
| Clamp | `cut.clamp(20hz..18khz)` |

`range(lo, hi)` and `clamp(x, lo, hi)` are removed. **[change]**

## 5. Functions

`fn` is the only way to define reusable code. The current `opcode` keyword is removed. **[change]**

### 5.1 Declaration

```
/// A one-pole low-pass filter.
fn one_pole(x, cutoff: hz) {
  state y = 0
  a = exp(-2 * pi * cutoff / sample_rate)
  y = x * (1 - a) + y * a
  y
}
```

```
fn name(parameters) -> results with(options) { body }
```

`-> results` and `with(...)` are both optional.

**Pure and stateful.** What a function becomes depends on its body, not on a keyword:

- A function **without state** is pure. It works at any rate: while compiling (building a table, a parameter default, score material), at note start, per block or per sample.
- A function **with state** (a `state` binding, `prev`, or a call to a stateful function) is a unit generator. Each call site has its own state, per voice. It can only be called where a voice or a track is running, never while compiling.

```
fn mtof(note: num) -> hz { 440hz * pow(2, (note - 69) / 12) }

table_freq = mtof(60)                    // while compiling: 261.6hz
instr lead(note: num) { osc(saw, mtof(note)) }   // per note, or per sample if note glides
```

**Definition order does not matter.** Functions may call functions defined later in the file. Recursion, direct or indirect, is an error that names the cycle. **[change]**

**Functions have no side effects** other than their own state. Sending to a bus (`space += ...`) is only allowed in instruments, tracks and buses. This keeps every function reusable anywhere. **[new]**

### 5.2 Parameters

```
name                        // any unit, any rate (generic)
name: type                  // required
name: type = default        // optional
name = default              // optional; type taken from the default
```

A **type** is an optional rate followed by what the value is:

```
type  = [rate] kind
rate  = const | init | control
kind  = unit | range | choice | list
```

| Kind | Example | Accepts |
| --- | --- | --- |
| Unit | `cutoff: hz` | A value of that unit |
| Range | `res: 0..1`, `cutoff: 20hz..20khz` | A value of the range's unit. Values outside the range are clamped, with a warning the first time |
| Choice | `mode: (low \| band \| high) = low` | One of the listed names; always `const` **[new]** |
| List | `coeffs: [num]`, `table: [num]` | A list whose entries have that kind; its length becomes `const` inside the body **[new]** |

**Rate qualifiers** say what the function needs from the caller **[new]**:

| Written | Inside the body, the parameter is | When the caller passes something faster |
| --- | --- | --- |
| *(none)* | Whatever was passed | Nothing changes |
| `control` | At most `control` | It is read once per block |
| `init` | At most `init` | Its value at note start is used |
| `const` | `const` | Compile error |

```
fn string(x, freq: init hz, size: const int = 4096) { ... }
```

**Untyped parameters are generic.** `fn twice(x) { x * 2 }` works on `num`, `hz` or `db`, at any rate, and the result's unit follows. Every call is compiled for the units and rates it is given. **[change]** (today an untyped parameter must be a plain number)

**Defaults** may use earlier parameters and must not be faster than their parameter's rate: `fn f(freq: hz, cutoff: hz = freq * 4)`. **[new]**

**Multichannel arguments.** A multichannel signal passed to a single-value parameter runs the function once per channel. A list parameter (`x: [num]`) receives the channels as a list instead, which is how a function processes stereo as a whole (mid/side, ping-pong):

```
fn mid_side(x: [num], width: 0..2 = 1) {
  [l, r] = x
  mid = (l + r) * 0.5
  side = (l - r) * 0.5 * width
  [mid + side, mid - side]
}
```

### 5.3 Results

- **The last expression** is the result. **[built]**
- **Or `out`:** a body may instead build `out` with `out = ...` and `out += ...`. `out` starts as silence. Using both forms in one body is an error. **[built]**
- **Result unit** is inferred. `-> hz` declares it and is checked. Results can have any unit. **[change]** (today an opcode can only output plain numbers)
- **Several results** are named after `->`. The body binds each name, and the caller reads them as fields. The body runs **once** for all results. **[change]** (today it runs once per result)

```
/// A state-variable filter with all three outputs.
fn svf(x, cutoff: hz, res: 0..1 = 0.5) -> (low, band, high) {
  state low = 0
  state band = 0
  f = 2 * sin(pi * cutoff / sample_rate)
  high = x - low - 2 * (1 - res) * band
  band = band + f * high
  low = low + f * band
}

f = noise().svf(cutoff: 2khz)
out = f.band
```

Used where a single value is expected, a function with several results gives its first one. **[built]**

### 5.4 State

```
state name = initial
```

- **Persists between samples,** per call site and per voice. **[built]**
- **The initial value** may be any `init` expression, including parameters: `state level = start`. **[change]** (today only a literal)
- **The unit** comes from what is assigned to it. A plain initial value such as `0` takes that unit. **[change]**
- **Reading and writing follow line order.** A read above an assignment in the body sees the previous sample's value. A read below it sees the new value. **[built]**
- **A state list** holds a buffer: `state buf = zeros(4096)`. Its length must be `const`. Entries are written with `buf[i] = value` (§7.3). **[new]**

**`prev(x)`** is the value `x` had one sample ago. `prev(y)` may appear before `y` is bound, which is how feedback is written:

```
fn comb(x, g: 0..1 = 0.5) {
  y = x + prev(y) * g
  y
}
```

`prev` means one sample everywhere inside a function or an instrument. An instrument that uses it is run sample by sample, like a function with state. **[change]** (today `prev` in an instrument is only allowed in a bus send, where it means one block)

## 6. Statements and expressions

### 6.1 Statements

| Statement | Meaning |
| --- | --- |
| `name = expr` | Bind or rebind a local. Reads see the most recent binding above them. Rebinding is never feedback |
| `[a, b] = expr` | Bind the entries of a list or the channels of a signal |
| `state name = expr` | Declare state (§5.4) |
| `name[i] = expr` | Write an entry of a state list |
| `out = expr`, `out += expr` | Build the result (§5.3) |
| `bus += expr` | Send to a bus; instruments, tracks and buses only |
| `if ... { } else { }` | Conditional statements (§6.4) |
| `for x in xs { }` | Repeated statements (§6.5) |
| `expr` | Only as the last line, where it is the result |

- **Reading an unbound name** is an error. A binding that is never read is a warning. **[built]**
- **A local can't reuse the name** of a top-level declaration (bus, instrument, function, table and so on). **[built]**
- **Scoping in blocks:** a block can read every name above it. Assigning to a name from outside the block updates that name. A name first bound inside a block stays inside it. **[new]**

### 6.2 Operators, by precedence

From the tightest-binding to the loosest:

| Level | Operators | Notes |
| --- | --- | --- |
| 1 | `f(...)`, `x.f`, `x[i]` | Calls, methods, indexing |
| 2 | `-x`, `not x` | |
| 3 | `*`, `/` | Division by zero gives 0 |
| 4 | `+`, `-` | |
| 5 | `..`, `..<` | Ranges |
| 6 | `<`, `>`, `<=`, `>=`, `==`, `!=` | Give `1` or `0`; no chaining (`a < b < c` is an error) |
| 7 | `and` | **[new]** |
| 8 | `or` | **[new]** |

- **Remainder is `x.mod(y)`.** The `%` operator is removed, so `%` only means percent and spacing never changes meaning. `mod` takes the sign of the divisor and gives 0 for a divisor of 0. **[change]**
- **There is no power operator.** Use `pow(x, y)`.
- **`and`, `or` and `not`** treat any non-zero value as true and give `1` or `0`. Both sides are always computed, because they may be signals. **[new]**
- **Comparisons** need both sides to have the same unit. Choice values compare with `==` and `!=`.

### 6.3 Math

These work everywhere: in instruments, functions, while compiling, and at any rate **[change]** (today `min`, `max`, `clamp` and `pow` only work inside opcodes):

- **One argument, plain numbers:** `sin cos tan tanh exp log sqrt abs sign floor ceil fract`.
- **Two values of one unit:** `min(a, b)`, `max(a, b)`.
- **Plain numbers:** `pow(x, y)`.
- **Ranges:** `x.clamp(range)`.
- **Constants:** `pi` and `sample_rate` (in hz).

### 6.4 `if`

**As an expression,** `if` needs an `else`. Each branch is a block whose last line is its value:

```
gain = if freq < 2khz { 0db } else if freq < 6khz { -6db } else { -14db }

shaped = if drive > 0.5 {
  hot = x * (1 + drive * 8)
  hot.tanh
} else {
  x
}
```

**As a statement,** the `else` is optional. A branch can rebind outer names and write state. A name that only one branch assigns keeps its old value on the other branch:

```
fn hold(x, trig) {
  state h = 0
  if trig > prev(trig) { h = x }        // sample and hold, on a rising edge
  h
}
```

**How it runs** **[change]** (today a branch must be one expression):

- A condition known while compiling picks its branch while compiling. The other branch is never built. This is how choice parameters work.
- A condition known at note start picks its branch at note start.
- Otherwise both branches are computed and the result is selected every sample. Writes to state and to state lists only happen on the taken branch.
- Both branches must give the same unit.

### 6.5 `for`

`for` repeats a body a number of times known while compiling. It iterates over a range or a list. **[change]** (today `for` is an expression only)

**As an expression,** it gives a list of each pass's value:

```
partials = for n in 1..8 { osc(sine, freq * n) / n }
partials.sum
```

**As a statement,** it repeats statements. Assigning to an outer name carries the value from one pass to the next:

```
for n in 1..8 { out += osc(sine, freq * n).gain(-3db * (n - 1)) }

acc = 0
for i in 0..<taps.len { acc = acc + taps[i] * hist[pos - i] }
```

There are no `while` or `loop` statements, so nothing can loop forever on the audio thread. A loop over signals builds one branch of the graph per pass. A loop over list entries inside a stateful function may be compiled as a real loop instead. Both behave the same.

### 6.6 Calls

- **Named arguments:** `lpf(x, cutoff: 800hz, res: 0.3)`. **[built]**
- **Positional arguments** fill a function's leading **required** parameters in order. Parameters with defaults are always named. **[change]** (today a user function takes only its first argument by position, while built-ins vary)

  ```
  osc(saw, 440hz)                 // wave, freq: both required
  sig.lpf(800hz)                  // x, cutoff: both required
  sig.lpf(800hz, res: 0.3)        // res has a default: named
  adsr(5ms, 120ms, 0.4, 600ms)    // all four required
  ```

- **Method calls:** `x.f(a)` is `f(x, a)`. Calls chain from left to right. **[built]**
- **No parentheses** are needed after a dot when there are no further arguments: `x.tanh`, `xs.sum`. A bare name is always a reference, never a call. **[built]**
- **After a dot,** a name resolves to a result field (`f.band`) first, then a function. If both fit, it is an error. **[built]**
- **A list passed where a single value is expected** maps the call over its entries and gives a list: `osc(saw, freq * [1, 1.01, 0.99])` is three oscillators. **[built]**

### 6.7 Anonymous functions

`fn(a, b) { a * b }` is a function value. It is used as an argument (`.map`) and is always `const`. **[built]** for one parameter.

`it` is a shorthand for one-parameter functions: an argument that mentions `it`, passed where a function is expected, is a function of `it`. **[new]**

```
ratios = (1..8).map(fn(n) { 1 / n })
ratios = (1..8).map(1 / it)
```

## 7. Lists, state lists and tables

There is one list type. Where it lives decides what you can do with it.

### 7.1 Lists

- **Written as** `[a, b, c]` or produced by a range, `for` or `.map`. **[built]**
- **The length** is always `const`. Entries can be anything: numbers, signals, ranges, other lists.
- **Arithmetic** works entry by entry (`[1, 2, 3] * 2`). Two lists combined this way must have the same length. **[built]**
- **Methods:** `.len`, `.sum`, `.map(f)`, `.indices` (the same as `0..<xs.len`), `.min`, `.max`. **[built]** for `len`, `sum` and `map`.
- **Indexing** counts from 0 and wraps: `xs[-1]` is the last entry and `xs[xs.len]` is the first. Indexing can never fail. **[change]**
  - A `const` index picks an entry while compiling. The entries can be anything.
  - A changing index reads an entry while running. The entries must share a unit. A list of signals indexed by a changing value is a switch between them:

    ```
    wave = [osc(saw, f), osc(square, f), osc(tri, f)][step]   // all three run; one is heard
    ```

- **Reading between entries:** `xs.at(pos, interp: linear)` reads at a fractional position, wrapping. `interp:` is `none`, `linear` or `cubic`. **[new]**
- **Slices:** `xs[0..<16]` takes entries with `const` bounds. **[new]**
- **Channels:** a list of single-channel signals becomes a multichannel signal where it reaches `out`, a bus or a channel parameter (at most 16 channels). `sig.channels` turns a signal back into a list of its channels. **[change]** (today this is `.split`, a name the spec also uses for `split(left: ..., right: ...)`)

### 7.2 Tables

A top-level binding of a pure expression is computed once, when the code loads, and shared by every voice. A list bound this way is a table. The `table` keyword is removed. **[change]**

```
saw8   = harmonics([1, -1/2, 1/3, -1/4], size: 4096)       // built-in constructor
sine1k = (0..<1024).map(sin(2 * pi * it / 1024))           // built in the language
hall   = file("irs/hall.wav")                              // a list of channels, each a list of samples
```

### 7.3 State lists

`state` in front of a list makes it writable, per call site and per voice. It is how a function keeps a buffer **[new]**:

```
state buf = zeros(size)          // size must be const
buf[pos] = y                     // writes wrap like reads
```

`zeros(n)` makes a list of `n` zeros. Any `const` list can also be the initial value (`state taps = [0, 0, 0, 0]`). State lists are allocated before the voice starts, never on the audio thread.

## 8. Instruments

```
/// A plucked saw with a filter that follows the note.
instr pluck(freq: hz, bright: 0..1 = 0.5) with(max: 16, steal: oldest) {
  level = env[0 2ms 1 600ms.exp 0]
  osc(saw, freq)
    .lpf(freq * (2 + bright * 6), res: 0.3)
    * level
}
```

An instrument is a function that the score can call: each note creates a voice that runs the body. Everything in §3–§7 applies, plus the rules below.

### 8.1 Kinds

| Keyword | A new note… |
| --- | --- |
| `instr` | Starts a new voice (polyphonic) **[built]** |
| `mono` | Ends the voice before it and starts its own; with `legato: true`, updates the running voice **[built]** |

### 8.2 Parameters

- Parameters follow §5.2, but **each needs a type or a default**, so the score can be checked against them. Parameters can't be generic. **[built]**
- **`const` isn't allowed,** because notes supply values while running. For a structural setting, write a `fn` and call it with a constant.
- **Without a rate,** a parameter stays live: lanes, glides, MPE and `n.set(...)` move it on the running voice.
- **`init`** fixes the parameter at note start, which is what a plucked string's length or a per-note random seed needs. **[new]**
- **`gain` is a universal control,** applied after the instrument. An instrument may declare its own `gain` parameter to take it over, for example to drive the filter harder on louder notes. The `amp` convention in the examples goes away. **[change]** (today declaring `gain` is an error)
- **`freq` is the pitch** by convention. `with(pitch: name)` changes it. **[built]**

### 8.3 Body and result

- **The result** is the last expression or `out` (§5.3). It must be a plain signal. It is converted to the master's channel layout. **[built]**
- **Sends:** `space += out.gain(-12db)` sends a copy to a bus. An instrument that only sends needs no result. **[built]**
- **Calling another instrument** gives its audio as a sub-voice of this voice: `out = pluck(freq: freq) + pluck(freq: freq * 1.5)`. **[new]**
- **Scheduling:** `schedule(echo(freq: freq * 1.5), in: 1/8 beat)` schedules a note through the score model. **[new]**

### 8.4 Note context

Inside an instrument body, `note` describes the running voice **[new]**:

| Name | Unit, rate | Meaning |
| --- | --- | --- |
| `note.time` | sec, audio | Time since the voice started |
| `note.dur` | sec, init | Scheduled duration; 0 for a note held from a keyboard |
| `note.released` | num, audio | 0 while held, 1 after release |

Functions don't see `note`. An instrument passes these values in as arguments, which keeps functions reusable.

### 8.5 Envelopes

`env[...]`, `adsr` and `perc` work as described in `niminal Spec.md` **[built]**. One change: steps may be names or parenthesised expressions, not only literals, so envelopes can follow parameters **[change]**:

```
instr pad(freq: hz, attack: sec = 400ms, release: sec = 2sec) {
  osc(saw, freq) * env[0 attack 1 | release 0]
}
```

Envelope steps are read at note start (`init`). A step that changes after that is held at its starting value.

### 8.6 Options

Options go in `with(...)`, right after the parameters, for every declaration that has them. **[built]** for instruments; tracks and clips move theirs to the same place.

| Option | Effect |
| --- | --- |
| `max: n, steal: oldest \| quietest` | Caps the number of voices |
| `choke: group` | A new note ends the other voices in the group |
| `same_note: release` | A new note releases the voice at the same pitch |
| `legato: true` (mono) | Overlapping notes update the running voice |
| `priority: last \| lowest \| highest` (mono) | Which held note sounds |
| `glide: 80ms` (mono) | Pitch changes slide over this time |
| `pitch: name` | Which parameter is the pitch |
| `tail: 2sec` | How long the voice may ring after release with no release envelope **[new]** |
| `quantize: 1 beat` | When a redefinition lands |

### 8.7 Voice lifetime

A voice ends when:

- it has been released and every envelope with a release has finished; or
- it has no release envelope, its `tail:` (default 0) has passed since release, and a short fade has finished; or
- it is choked, stolen or replaced. A short fade always prevents clicks.

**[change]** Today a voice without an envelope that runs through a delay never ends: it rendered 60 seconds instead of 0.5. Ringing effects belong on a track or bus, or need an explicit `tail:`.

## 9. Worked examples

### A user-written filter with a mode switch

```
fn filter(x, cutoff: hz, res: 0..1 = 0.5, mode: (low | band | high) = low) {
  f = x.svf(cutoff: cutoff, res: res)
  if mode == low { f.low } else if mode == band { f.band } else { f.high }
}

instr bass(freq: hz) {
  osc(saw, freq).filter(freq * 4, mode: band) * perc(decay: 300ms)
}
```

`mode` is `const`, so the unused branches are never built, and `svf` runs once.

### Karplus-Strong from scratch

```
/// A plucked string: a delay line fed back through an averaging low-pass.
fn string(x, freq: init hz, decay: 0..1 = 0.996, size: const int = 4096) {
  state buf = zeros(size)
  state pos = 0
  len = sample_rate / freq                          // once per note
  delayed = buf.at(pos - len, interp: linear)
  y = x + (delayed + prev(delayed)) * 0.5 * decay
  buf[pos] = y
  pos = (pos + 1).mod(size)
  y
}

instr pluck(freq: init hz, decay: 0..1 = 0.996) with(tail: 3sec) {
  (noise() * env[1 5ms 0]).string(freq, decay: decay)
}
```

### A wavetable oscillator from a table

```
saw8 = harmonics([1, -1/2, 1/3, -1/4], size: 4096)

fn phasor(freq: hz) {
  state p = 0
  p = (p + freq / sample_rate).fract
  p
}

fn wavetable(table: [num], freq: hz) {
  table.at(phasor(freq) * table.len, interp: linear)
}

instr wt(freq: hz) {
  wavetable(saw8, freq) * adsr(5ms, 100ms, 0.6, 300ms)
}
```

### A detuned stack, using lists

```
instr supersaw(freq: hz, detune: 0..1 = 0.3) {
  ratios = for i in 0..<7 { 1 + (i - 3) * detune * 0.01 }
  osc(saw, freq * ratios).sum / 7 * adsr(10ms, 200ms, 0.7, 400ms)
}
```

### An organ, built with a statement loop

```
instr organ(freq: hz) {
  for n in 1..8 { out += osc(sine, freq * n).gain(-3db * (n - 1)) }
}
```

### A pure helper used at every rate

```
fn mtof(note: num) -> hz { 440hz * pow(2, (note - 69) / 12) }

low_c = mtof(36)                                  // const: computed while compiling
instr keys(note: num) { osc(tri, mtof(note)) }    // follows the note, gliding if it glides
```

### An FIR filter

```
fn fir(x, taps: [num]) {
  state hist = zeros(taps.len)
  state pos = 0
  hist[pos] = x
  acc = 0
  for i in taps.indices { acc = acc + taps[i] * hist[pos - i] }
  pos = (pos + 1).mod(taps.len)
  acc
}

smooth = [0.25, 0.5, 0.25]
instr soft(freq: hz) { osc(square, freq).fir(smooth) }
```

## 10. Reserved words

Only the words used by this part of the language are listed here. The full list stays in `niminal Spec.md`.

| Kind | Words |
| --- | --- |
| Declarations | `fn`, `instr`, `mono`, `state`, `with` |
| Control flow | `if`, `else`, `for`, `in` |
| Logic | `and`, `or`, `not`, `true`, `false` |
| Rates | `const`, `init`, `control` |
| Implicit names | `out`, `it`, `note` |
| Constants | `pi`, `sample_rate` |
| Units | `num`, `int`, `sec`, `ms`, `samples`, `hz`, `khz`, `db`, `st`, `deg`, `bpm`, `beat`, `beats`, `bar`, `bars`, `%` |

`opcode` and `table` are no longer reserved.

## 11. Summary of changes from the current implementation

| Area | Today | This spec |
| --- | --- | --- |
| Keywords | `opcode` and `fn` | `fn` only; state makes it a unit generator |
| Untyped parameters | Plain numbers | Generic: any unit, any rate |
| Positional arguments | First only (user); varies (built-ins) | Leading required parameters, for everything |
| Results | Plain numbers only | Any unit; `-> hz` |
| Several results | Body runs once per result | Body runs once |
| Definition order | Opcodes must come before their callers | Any order; recursion is an error |
| State initial value | A literal | Any `init` expression |
| Ranges | `..` includes the end in types, excludes it in lists | `..` includes the end everywhere; `..<` excludes it |
| `clamp`, `range` | Two spellings | Take a range only |
| Remainder | `%` operator, sensitive to spacing | `x.mod(y)` |
| Logic | Multiply comparisons together | `and`, `or`, `not` |
| `if` | One expression per branch | Blocks; statement form; state writes |
| `for` | Expression only | Also a statement; carries assignments between passes |
| Math | `min`, `max`, `clamp`, `pow` only in opcodes | Everywhere |
| `prev` | One sample in opcodes; bus sends only in instruments | One sample everywhere inside a voice |
| Lists | Compile-time only | Indexing while running, wrapping, `.at`, slices, state lists |
| Tables | `table` keyword (not built) | Top-level bindings |
| Rates | Not available | `const`, `init`, `control` on parameters; `.init`, `.control`; hoisting |
| Choice parameters | Built-ins only (`wave`) | `mode: (a \| b \| c)` for user functions |
| Envelope steps | Literals only | Names and expressions, read at note start |
| `gain` | Reserved; instruments use `amp` | Universal; an instrument may take it over |
| Note context | None | `note.time`, `note.dur`, `note.released` |
| Channels to list | `.split` | `.channels` |
| Voice lifetime | No envelope plus a delay never ends | `tail:` option; ends after release |
| Docs | None for user code | `///` doc comments |

**Implementation note.** Today two compilers handle instruments (`lower.rs`, a block graph) and opcodes (`kernel.rs`, per-sample code). They duplicate argument binding, unit checks, `if` and comparisons, which is why some features only exist on one side. This spec assumes a single front end that infers the unit, rate and shape of every value. Two back ends would follow from it: a block graph for stateless chains, and fused per-sample code wherever there is state or feedback.

## 12. Open questions

1. **Positional arguments.** "Leading required parameters" relaxes the main spec's "positional only for an obvious first argument". It makes built-ins and user functions consistent, and it is shorter live (`sig.lpf(800hz)`). The alternative is to keep the first-only rule and change `osc` and `adsr` to match.
2. **Bus feedback.** Inside a voice, `prev` is now one sample. Feedback between buses (`echo += prev(out)`) is still one block, because buses run in blocks. Should the bus form get its own name, such as `out.late`, so that `prev` always means one sample?
3. **Functions that build score material.** Brackets in a body are lists, so a `fn` can't write a pattern (`[c2 ~ eb2]`). Score functions (`fn verse(...)` in the main spec) need one. Options include a pattern prefix inside bodies (`p[c2 ~ eb2]`) or letting the expected type decide.
4. **Functions as parameters.** Should a `fn` take a function argument (`fn layer(n: const int, make: fn) { ... }`)? It is cheap to add because functions are `const`.
5. **`match`.** `if mode == low { } else if ...` works for choices. A `match mode { low: ..., band: ... }` would read better once choices have more than three options.
6. **Annotations on bindings** (§3.2). They are left out for now and could be added with the parameter syntax if dogfooding asks for them.
