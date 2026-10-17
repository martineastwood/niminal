# niminal language: functions and instruments

Built · Oct 13, 2026

This document defines the core of the niminal language: values, functions (`fn`) and instruments (`instr`, `mono`). It is the base that every opcode is built on, so it came before growing the opcode library. It replaces the "Orchestra" sections of `niminal Spec.md` that cover syntax, opcodes, functions and instruments. Tracks, clips, scenes, patterns and the score model are out of scope here; they keep their own sections and reuse the statement and expression rules below. Score helpers use the separate keyword `def` (not `fn`) so the DSP checker never sees them — see the Performance reference for `def`, top-level `for` / `if`, and pattern combinators.

Everything here is built unless it is marked **[not built]**. Section 11 lists what changed from the first draft while it was built, and why.

## 1. Principles

1. **One way to say a thing.** Each concept has one spelling. When two forms compete, one goes.
2. **Concise for the stage, readable at length.** Short where it is typed live (`sig.lpf(800hz)`), explicit where it is read later (named arguments, units).
3. **Inferred by default, declared at the boundaries.** Units, rates and shapes are inferred inside a body. Parameters say what a function needs.
4. **Nothing on the audio thread can fail.** No unbounded loops, no out-of-bounds reads, no division errors.
5. **Braces, never indentation.** Any selected block can be evaluated on its own.
6. **Built-ins and user code look the same.** Calls, named arguments, units and errors work the same for both.

## 2. Lexical structure

- **Comments:** `//` to the end of the line. A `//` or `///` comment directly above a `fn` or `instr` is its documentation, shown on hover.
- **Statements** end at a newline. Newlines inside `( )` and `[ ]` are ignored. A line starting with `.` continues the previous expression.
- **Blocks** are `{ ... }`. Indentation never matters.
- **Names** are `snake_case`. A top-level name starting with `_` is private to its module.
- **Numbers:** `0.5`, `440`, `-3`, and rationals with an attached unit (`1/8beat`).
- **Units attach directly** to a literal (`440hz`, `-6db`, `5ms`). Never after a space: inside brackets a space separates steps, and outside them `3/16 beat` would divide by `16beat`. To give an expression a unit, use the unit method: `(n * 2).bars`.
- **Strings:** `"drums/kick.wav"`. They are used only for paths and addresses.
- **Brackets** in a body (`fn`, `instr`, track or bus), in a signature, or after `const` are lists. Elsewhere at the top level they are patterns (`[c2 ~ eb2]`), which belong to the performance layer.

## 3. Values

Every value has three properties, all inferred:

| Property | Answers | Examples |
| --- | --- | --- |
| **Unit** | What it measures | `num`, `hz`, `db`, `sec`, `st`, `deg` |
| **Rate** | How often it changes | `const`, `init`, `control`, `audio` |
| **Shape** | How many of it there are | a single value, a list, a multichannel signal |

### 3.1 Units

- Plain numbers (`num`) are unitless. Oscillators, envelopes and LFOs produce `num`.
- `num × hz = hz`, `hz + hz = hz`, `hz / hz = num`. Adding values of different units is an error.
- Related units convert: note names to hz (`c4`), semitones or cents (`ct`, hundredths of a semitone) applied to a frequency (`freq + 7st`, `freq + 14ct`), beats to seconds at the current tempo, and db as a gain.
- `x.num` strips a unit and `x.hz` (or `.khz`, `.db`, `.sec`, `.ms`, `.beats`, `.bars`, `.st`, `.ct`, `.deg`) gives a plain value one.
- A bare number where a unit is expected is an error with a quick fix (`cutoff: 800` → "did you mean `800hz`?"). Gain parameters also accept a plain linear factor.
- **`int`** is a `num` that must be a whole number. It is used for sizes, counts and indices.
- **`true` and `false`** are `1` and `0`.

### 3.2 Rates

A rate says how often a value can change. There are four, from slowest to fastest:

| Rate | Csound | Changes | Typical uses |
| --- | --- | --- | --- |
| `const` | (orchestra load) | Never: known while compiling | List sizes, loop counts, a delay's `max`, `mode:` choices, tables |
| `init` | i-rate | Once, at note start | A plucked string's length, a random seed per note |
| `control` | k-rate | Once per block (32 samples) | Slow modulation, filter coefficients when CPU matters |
| `audio` | a-rate | Every sample | Signals, audio-rate modulation |

**Inference.** A value's rate is the fastest rate among its inputs:

- Literals, `pi` and tables are `const`; `sample_rate` is `init`.
- A note's parameters are `audio` unless declared otherwise (§8.2), because lanes, glides and MPE can move them. Controls (`ctl`) are `control`.
- Oscillators, envelopes and state are `audio`.
- `freq * 2` has the rate of `freq`. `exp(-2 * pi * cutoff / sample_rate)` has the rate of `cutoff`.

**Each value is worked out at its own rate.** In a function with state, if `cutoff` only changes per note, the `exp(...)` above runs once per note, though the function otherwise runs every sample. This is what Csound's i/k/a prefixes were for, and it needs no syntax. Outside functions with state, everything runs once per block of samples anyway.

**Reading a value more slowly.** A value never needs to be made faster: an `init` value feeds an audio input as is. To make one slower, use a method:

| Method | Gives |
| --- | --- |
| `x.init` | The value `x` had at note start (Csound's `i(k)`) |
| `x.control` | `x` read at the start of each block and held |

**Rate requirements are checked.** Passing a value that changes where `const` is needed is an error: "`size` must be known while compiling".

#### Why bindings carry no rate or type annotations

Writing `init x = 0.5` or `control y = 5hz` would add a third way to state a rate, after parameter types and the `.init` and `.control` methods. Most of the time it would only repeat what is already known: `0.5` and `5hz` are literals, so they are already `const`.

A local's rate is decided by its inputs. The two reasons to change it are both covered by methods, which also work inline:

```
start = freq.init                       // freeze at note start
wobble = osc(sine, 5hz).control         // cheaper: once per block
osc(sine, freq.init)                    // inline, no binding needed
```

Parameters are where annotations earn their place, because they are the contract with the caller (§5.2). If dogfooding shows that local annotations help readability, the parameter syntax can be allowed on bindings later (`cut: hz = ...`). Until then, bindings stay `name = expr`.

### 3.3 Shapes: values, arrays and channels

- **A value** is one number or one signal.
- **An array** has a length known while compiling. Elements can be constants, changing values or nested arrays (§7). Tables and writable buffers specialize its storage.
- **A multichannel signal** specializes an array of channels with a rate, a unit and a channel layout (`stereo`, `surround(5.1)`, …). Arrays become audio channels at `out`, a bus, `pan` or layout conversion. Signals support array operations directly; array sizes are independent of channel limits.

## 4. Ranges

A range is a value with two ends, written with `..`:

| Form | Meaning |
| --- | --- |
| `a..b` | From `a` to `b`, **including** `b` |
| `a..<b` | From `a` up to, not including, `b` |

- **Ranges carry units:** `20hz..20khz`, `-12db..0db`, `0..1`. Both ends must have the same unit.
- **Ranges are values:** `sweep = 200hz..4khz` can be passed around and reused.
- **Whole-number ranges iterate:** `for n in 1..8` runs eight times and `for i in 0..<n` runs `n` times.
- **One spelling everywhere.** Every function that needs two ends takes a range, never a `lo, hi` pair:

| Use | Example |
| --- | --- |
| Bounded parameter type | `bright: 0..1`, `cutoff: 20hz..20khz` |
| Loop | `for i in 0..<8 { ... }` |
| Slice | `buf[0..<16]` |
| Map a 0..1 signal | `knob.range(200hz..4khz)` |
| Clamp | `cut.clamp(20hz..18khz)` |
| Audio input channels | `input(1..2)` |

`(0..1).step(0.25)` is the array `0, 0.25, 0.5, 0.75, 1` for ranges that aren't whole numbers (`(0..<1).step(0.25)` leaves the end out). The step is above 0 and in the range's unit: `(200hz..400hz).step(100hz)`.

## 5. Functions

`fn` is the only way to define reusable code. There is no `opcode` keyword.

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
fn name(parameters) -> results { body }
```

`-> results` is optional.

**Pure and stateful.** What a function becomes depends on its body, not on a keyword:

- A function **without state** is pure. It works at any rate: while compiling (a table, a constant), at note start, per block or per sample. Where it is called its body becomes part of the caller.
- A function **with state** (a `state` line, `prev`, a write to a state list, or a call to a function that has one) is a unit generator. Each call has its own state, per voice, and runs sample by sample. It can't run while compiling.

Each call is compiled for the units and rates it is given, so one function serves many purposes:

```
fn mtof(n: num) -> hz { 440hz * pow(2, (n - 69) / 12) }

const low_c = mtof(36)                           // while compiling: 65.4hz
instr keys(key: num) { osc(tri, mtof(key)) }      // follows the note, gliding if it glides
```

**Definition order does not matter.** Functions may call functions defined later in the file. A function that calls itself, directly or through others, is an error that names the cycle.

**Functions have no side effects** other than their own state. Sending to a bus, `osc_out`, `input` and `note` are only for instruments, tracks and buses. This keeps every function reusable anywhere.

**Mistakes are found where a function is defined**, even if nothing calls it. A function with untyped parameters is checked with a guess at their units (and list parameters at their lengths); a mistake that could come from the guess is only reported where the function is called.

### 5.2 Parameters

```
name                        // any unit, any rate (generic)
name: type                  // required
name: type = default        // optional
name = default              // optional; type from the default
```

A **type** is an optional rate followed by what the value is:

```
type  = [rate] kind
rate  = const | init | control
kind  = unit | range | choice | list | fn
```

| Kind | Example | Accepts |
| --- | --- | --- |
| Unit | `cutoff: hz` | A value of that unit: `num`, `int`, `hz`, `db`, `sec`, `st`, `deg` |
| Range | `res: 0..1`, `cutoff: 20hz..20khz` | A value of the range's unit. A value known while compiling must be inside it; one that changes is clamped |
| Choice | `mode: (low \| band \| high) = low` | One of the listed names; always `const`. Inside the body each name stands for its choice, so `mode == low` picks a branch while compiling |
| Function | `make: fn` | A user function name or anonymous function, known while compiling |
| List | `coeffs: [num]`, `table: [num]` | A list whose entries have that kind; its length is `const` inside the body. A multichannel signal passed here is the list of its channels |

**Rate qualifiers** say what the function needs from the caller:

| Written | Inside the body, the parameter is | When the caller passes something faster |
| --- | --- | --- |
| *(none)* | Whatever was passed | Nothing changes |
| `control` | At most `control` | It is read once per block |
| `init` | At most `init` | Its value at note start is used |
| `const` | `const` | Compile error |

```
fn string(x, freq: init hz, size: const int = 4096) { ... }
```

A rate may be written without a kind: `rate: control = 10hz`.

**Untyped parameters are generic.** `fn twice(x) { x * 2 }` works on `num`, `hz` or `db`, at any rate, and the result's unit follows.

**Defaults** may use earlier parameters: `fn f(freq: hz, cutoff: hz = freq * 4)`.

**Multichannel arguments.** A list parameter (`x: [num]`) receives a signal's channels as a list, which is how a function processes stereo as a whole:

```
fn mid_side(x: [num], width: 0..2 = 1) {
  [l, r] = x
  mid = (l + r) * 0.5
  side = (l - r) * 0.5 * width
  [mid + side, mid - side]
}
```

A multichannel signal passed to a function with state where one value is expected runs one copy of it per channel. A function without state works on all the channels at once, as arithmetic does.

### 5.3 Results

- **The last expression** is the result.
- **Or `out`:** a body may instead build `out` with `out = ...` and `out += ...`. `out` starts as silence. Using both forms in one body is an error.
- **The result's unit** is inferred. `-> hz` declares it, and it is checked.
- **Several results** are named after `->`. The body binds each name, and the caller reads them as fields. The body runs **once** for all of them.

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

f = osc(saw, 110hz).svf(cutoff: 2khz)
out = f.band
```

Used where a single value is expected, a function with several results gives its first one.

### 5.4 State

```
state name = initial
```

- **Persists between samples,** per call and per voice.
- **The initial value** is worked out at note start and may use the parameters: `state y = x` starts at the input's first sample.
- **The unit** is the initial value's unit: `state y = 0` holds plain numbers, `state y = 0hz` frequencies, and `state y = x` whatever `x` is. Assigning a value of another unit is an error.
- **Reading and writing follow line order.** A read above an assignment in the body sees the previous sample's value. A read below it sees the new value.
- **A state list** holds a buffer: `state buf = zeros(4096)` or `state taps = [0, 0, 0, 0]`. Its length must be `const`. Entries are written with `buf[i] = value` (§7.3).
- **State is declared at the top of a function,** not inside a block.

**`prev(x)`** is the value `x` had one sample ago. `prev(y)` may appear before `y` is bound, which is how feedback is written; the value read that way is a plain number:

```
fn comb(x, g: 0..1 = 0.5) {
  y = x + prev(y) * g
  y
}
```

`prev` means one sample everywhere inside a function or an instrument. In an instrument, `y = ... prev(y) ...` on one line becomes a small function with state whose inputs are the names the line reads. A loop through several lines needs a `fn`. In a bus send, `prev` is an error: `late(x)` is a send that arrives a block late, because buses run in blocks (`echo += late(out)`).

### 5.5 Reset scopes and completion

`reset_on(trigger) { ... }` restarts all state created inside its body on positive trigger edges, before processing that sample. Initially positive triggers reset at the first sample; sustained positive levels do not repeat. Nested functions, built-ins, `prev`, state arrays and initialization restart, with current inputs used by initializers. Values calculated outside keep running. Nested scopes reset with their parent. Storage is allocated before rendering; resets do not allocate. The body has local bindings and gives its last expression or its own `out`. Stateful bodies follow function context rules; capture note context in a value outside.

`signal.finish_when(done)` latches completion when `done` becomes positive, silences that layer, and participates in envelope lifetime rules. It works inside functions and instruments. Voices still wait for release, every envelope/completion signal, and downstream effect tails; `tail:` still applies. Resetting the containing scope clears completion. Choose a completion point where the layer is silent.

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

- **Reading an unbound name** is an error.
- **A local can't reuse the name** of a top-level declaration (bus, instrument, function, constant and so on). Inside a function only the names it could read count: buses, controls, functions and constants.
- **Scoping in blocks:** a block can read every name above it. Assigning to a name from outside the block updates that name. A name first bound inside a block stays inside it.

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
| 7 | `and` | |
| 8 | `or` | |

- **Remainder is `x.mod(y)`.** There is no `%` operator, so `%` only means percent and spacing never changes meaning. `mod` takes the sign of the divisor and gives 0 for a divisor of 0.
- **There is no power operator.** Use `pow(x, y)`.
- **`and`, `or` and `not`** treat any non-zero value as true and give `1` or `0`. Both sides are always worked out, because they may be signals.
- **Comparisons** need both sides to have the same unit. Choice values compare with `==` and `!=`.

### 6.3 Math

These work everywhere: in instruments, functions, while compiling, and at any rate:

- **One argument, plain numbers:** `sin cos tan tanh exp log sqrt abs sign floor ceil fract`.
- **Two values of one unit:** `min(a, b)`, `max(a, b)`.
- **Plain numbers:** `pow(x, y)`.
- **Ranges:** `x.clamp(range)`.
- **Constants:** `pi` and `sample_rate` (in hz).

A call whose arguments are known while compiling is worked out then.

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

**As a statement,** the `else` is optional. A branch can rebind outer names, `out` and state. A name that only one branch assigns keeps its old value on the other branch:

```
fn hold(x, trig) {
  state h = 0
  if trig > 0 and prev(trig) <= 0 { h = x }     // sample and hold, on a rising edge
  h
}
```

**How it runs:**

- A condition known while compiling picks its branch while compiling. The other branch is never built. This is how choice parameters work.
- Otherwise both branches are worked out and the result is selected every sample. In a function with state, writes to state and to state lists only happen on the taken branch, including the state of functions called inside the branch.
- Both branches must give the same unit.
- A bus send inside an `if` that changes while playing is an error, because it would always be made: send the `if` instead.

**`match`** is the `else if` chain on one name, for choices and for numbers. An arm is a name (or several, with `|`) and a value or a block; `else` is the rest:

```
fn pick(f, mode: (low | band | high) = low) {
  match mode {
    low: f.low
    band: f.band
    high: f.high
  }
}
```

As a value it needs an `else`, as `if` does. The thing matched is a name, so it is tested once per arm at no cost.

### 6.5 `for`

`for` repeats a body a number of times known while compiling. It iterates over a range, a list or a table.

**As an expression,** it gives a list of each pass's value:

```
partials = for n in 1..8 { osc(sine, freq * n) / n }
partials.sum
```

**As a statement,** it repeats statements. Assigning to an outer name carries the value from one pass to the next:

```
for n in 1..8 { out += osc(sine, freq * n).gain(-3db * (n - 1)) }

acc = 0
for i in taps.indices { acc = acc + taps[i] * hist[pos - i] }
```

There are no `while` or `loop` statements, so nothing can loop forever on the audio thread. A loop builds one copy of its body per pass.

### 6.6 Calls

- **Named arguments:** `lpf(x, cutoff: 800hz, res: 0.3)`.
- **Positional arguments** fill a callee's leading **required** parameters in order (and always the first, the receiver of a method call). Parameters with defaults are named. This holds for built-ins and user functions alike:

  ```
  osc(saw, 440hz)                 // wave, freq: both required
  sig.lpf(800hz)                  // x, cutoff: both required
  sig.lpf(800hz, res: 0.3)        // res has a default: named
  adsr(5ms, 120ms, 0.4, 600ms)    // all four required
  ```

- **Method calls:** `x.f(a)` is `f(x, a)`. Calls chain from left to right.
- **No parentheses** are needed after a dot when there are no further arguments: `x.tanh`, `xs.sum`. A bare name is always a reference, never a call.
- **After a dot,** a name resolves to a result field (`f.band`) first, then a function.
- **An array given to a built-in** expands the built-in element by element and returns an array: `osc(saw, freq * [1, 1.01])` is two oscillators. `.sum` mixes a signal's channels down. A list given to a user function where one value is expected stays a list, and the body works on it entry by entry.

### 6.7 Anonymous functions

`fn(a) { ... }` is a function value. It is given to `.map`, and its body is a block.

`it` is a shorthand for one-parameter functions: an expression passed to `.map` that isn't a `fn(...)` is a function of `it`.

```
ratios = (1..8).map(fn(n) { 1 / n })
ratios = (1..8).map(1 / it)
```

User functions accept function parameters: `fn apply(x, make: fn) { make(x) }`. Arguments are user function names or anonymous functions. Anonymous functions capture surrounding values; each invocation owns independent state. Function values can be forwarded and used with `.map`. They are known while compiling, with no runtime dispatch. Their bodies are checked at specialization, including argument counts and units; functions accepting callbacks are checked at calls rather than with guessed callbacks. `init` and `control` do not apply to function parameters.

## 7. Arrays, tables and state buffers

There is one fixed-length array model. Tables, computed arrays and writable buffers share its operations; storage does not determine which operations are available.

### 7.1 Arrays

- `[a, b]`, `for` and `.map` make arrays. Values may be constant, changing, or nested arrays. Function parameter types such as `[num]`, `[hz]` and `[[num]]` check their elements.
- Arithmetic, comparisons, math, `.range`, `.clamp` and unit conversion apply element by element. Scalars broadcast; arrays must have matching shapes. Built-in DSP operations expand over arrays and return arrays, without imposing channel limits.
- `.len`, `.sum`, `.min`, `.max`, `.map(f)`, `.indices` and slicing work for every storage type. Nested reductions combine corresponding elements of the rows.
- `xs[i]` counts from zero, floors fractional indices and wraps in both directions. Changing indices require matching element shapes and units. `xs[a..<b]` copies selected values into an array.
- `xs.at(pos, interp: linear)` interpolates a one-dimensional numeric array, whether its values come from a table, a computed frame or a state buffer. Interpolation is `none`, `linear` (default) or `cubic`.
- Empty arrays (`[]`, `zeros(0)`, empty ranges and slices) support length, mapping and iteration. Indexing, interpolation and reductions on them are compile errors.
- Signals offer array operations directly (`signal[0]`, `signal.map(it * 0.5)`, `signal.sum`). Mapping and element-wise operations preserve channel layout. `.channels` gives an ordinary array of channels.
- Array arguments and results preserve their shape across pure and stateful functions, including arrays larger than the audio channel limit and named array results. `pan`, layout conversion and audio outputs consume channels and enforce channel constraints.

### 7.2 Constant tables

`const name = expr` evaluates an array while compiling. Numeric table storage is shared by voices. Constants may use array arithmetic, mapping, reductions, slicing, indexing and interpolation, plus functions without state.

```nml
const saw8 = harmonics([1, -1/2, 1/3, -1/4], size: 4096)
const sine1k = (0..<1024).map(sin(2 * pi * it / 1024))
const taps = [0.25, 0.5, 0.25]
```

`osc(saw8, freq)` plays a table as one cycle of a wave, band-limited at any pitch. The table is numbers known while compiling, from `harmonics`, a literal or `.map`; it can be up to 65536 entries.

`file("irs/hall.wav")` loads one channel of a WAV as a table. A `.csv`, `.txt` or `.dat` file of numbers is a table for each column: `file("curve.csv", channel: 1)` (a first line that isn't numbers is a header, `#` starts a comment). A path may hold constants: `file("kick{n}.wav")`, or `{n:2}` to pad to two digits. See the tables reference for the full set of constructors, transforms and shared `buffer` declarations.

### 7.3 State arrays

`state` declares a fixed-length writable numeric buffer, per call and per voice. It may start from constants or note-start values:

```nml
state buf = zeros(size)
state initial = [x, x * 2]
buf[pos] = y
```

Writes wrap like reads. Reads after a write see the new value. Mapping, slicing, passing and returning a state buffer read snapshots of its current elements. Ordinary array values and function arguments cannot be written. Array storage is allocated before audio processing and never resized while running.

### 7.4 Shared buffers

`buffer name = zeros(n)` or `buffer name = 4bars` declares a writable table that outlives voices, shared across instruments and tracks. Write with `buf[i] = x` or `buf.record(sig, loop:)`; read with `[i]`, `.at`, `.phase`, or play via a sample. Functions cannot write shared buffers. Live re-evaluation keeps contents when the name and length are unchanged. Tracks that write a buffer run before tracks that read it in the same block.

**State that notes share.** A buffer is how one note sees another, or one instrument another: a write in an `if` happens only while the branch is taken, so `if note.start { count[0] = count[0] + 1 }` counts notes, and `note.index` picks a slot per note. Within a track a writing voice starts before one that reads it only if it was started first, so write from the earlier note.

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
| `instr` | Starts a new voice (polyphonic) |
| `mono` | Ends the voice before it and starts its own; with `legato: true`, updates the running voice |

### 8.2 Parameters

- Parameters follow §5.2, but **each needs a type or a default**, so the score can be checked against them. Parameters can't be generic.
- **Structural parameters** (`const`, choices and functions) need defaults and specialize the graph. Embedded instrument calls can override them. Numeric arrays with defaults have a fixed shape and live elements. `buffer` parameters select prepared shared resources; both can be updated through the event API.
- **Without a rate,** a parameter stays live: lanes, glides, MPE and `n.set(...)` move it on the running voice.
- **`init`** fixes the parameter at note start, which is what a plucked string's length or a per-note random seed needs.
- **`gain` is a universal control,** applied after the instrument. An instrument may declare its own `gain: db` parameter to take it over, for example to drive the filter harder on louder notes; it is then not applied for it.
- **`freq` is the pitch** by convention. `with(pitch: tone)` makes the `hz` parameter `tone` the one a clip's notes, keys and `freq:` in a score set instead. A note call names the parameter directly: `a(tone: a5)`.

### 8.3 Body and result

- **The result** is the last expression or `out` (§5.3). It must be a plain signal. It is converted to the master's channel layout.
- **Sends:** `space += out.gain(-12db)` sends a copy to a bus. An instrument that only sends needs no result.
- **Calling another instrument** plays its body as part of this voice: `out = click(freq: freq) + click(freq: freq * 1.5, gain: -6db)`. Its `gain:` is applied after it, as for a note.
- **`after(delay) { body }`** runs `body` from when the note is `delay` old, with fresh state, and is silent until then: `out = pluck + after(1/8beat) { pluck.gain(-6db) }`. It is `reset_on(note.time >= delay) { body } * (note.time >= delay)`, so the body is computed (and any `state` in it is held) from the start. `after` is a reserved word.
- **Ending a note from inside:** `signal.finish_when(done)` (§5.5) is `turnoff`: the note ends once `done` is positive and everything else has finished.
- **Not scheduling.** A voice can't start another note through the score model: the audio thread allocates nothing, and voices are prepared ahead of time. Use `after` for delayed layers, a clip or `at` for notes, and shared buffers (§7.4) for state that notes share.

### 8.4 Note context

Inside an instrument body, `note` describes the running voice:

| Name | Unit, rate | Meaning |
| --- | --- | --- |
| `note.time` | sec, audio | Time since the voice started |
| `note.released` | num, audio | 0 while held, 1 after release |
| `note.dur` | sec, init | Scheduled duration (0 for a note held from a key, which has no end yet) |
| `note.start` | num, audio | 1 for the first sample of the voice, then 0 |
| `note.index` | num, init | How many notes of this instrument started before this one |
| `clock.beat` | num, control | Beats since the performance started (a reset sets it back to 0) |
| `clock.bar` | num, control | Bars, in the current meter |
| `clock.bpm` | num, control | The current tempo |

`clock` is the shared transport, so a voice can follow the tempo. Functions don't see `note` or `clock`. An instrument passes these values in as arguments, which keeps functions reusable. `note` is reserved, so it can't be a parameter or local name.

### 8.5 Envelopes

`env[...]`, `adsr` and `perc` work as described in `niminal Spec.md`. Steps may also be names or parenthesised expressions, so envelopes can follow parameters:

```
instr pad(freq: hz, attack: sec = 400ms, release: sec = 2sec) {
  osc(saw, freq) * env[0 attack 1 | (release * 2).exp 0]
}
```

Steps that aren't numbers are read at note start. Up to 16 steps of one envelope can be names or expressions. `adsr` and `perc` take them too: `adsr(5ms, 120ms, 0.4, release)`. Lane and tempo envelopes stay numbers only, since they have no note to read from.

### 8.6 Options

Options go in `with(...)`, right after the parameters.

| Option | Effect |
| --- | --- |
| `max: n, steal: oldest \| quietest` | Caps the number of voices |
| `choke: group` | A new note ends the other voices in the group |
| `same_note: release` | A new note releases the voice at the same pitch |
| `legato: true` (mono) | Overlapping notes update the running voice |
| `priority: last \| lowest \| highest` (mono) | Which held note sounds |
| `glide: 80ms` (mono) | Pitch changes slide over this time |
| `tail: 2sec` | How long the voice keeps sounding after its release (§8.7) |
| `quantize: 1beat` | When a redefinition lands |
| `pitch: name` | The `hz` parameter that notes set (default `freq`) |

### 8.7 Voice lifetime

A voice ends:

- **with an envelope or a sample in it:** once it has been released, every envelope has finished and anything they fed (a delay, a reverb) has rung out. With a `tail:`, it keeps sounding until its envelopes are done and the tail has passed since the release, then fades out. A plucked string, whose envelope is only a short burst of noise, needs one;
- **with neither:** at its release, after its `tail:` (default 0), with a short fade;
- **when it is choked, stolen or replaced.** A short fade always prevents clicks.

## 9. Worked examples

These are in `examples/functions.nml`, which also plays them.

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
/// White noise: each call has its own.
fn noise() {
  state s = 22222
  s = (s * 16807).mod(2147483647)
  s / 2147483647 * 2 - 1
}

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
const saw8 = harmonics([1, -1/2, 1/3, -1/4], size: 4096)

instr wt(freq: hz) {
  osc(saw8, freq) * adsr(5ms, 100ms, 0.6, 300ms)
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
fn mtof(n: num) -> hz { 440hz * pow(2, (n - 69) / 12) }

const low_c = mtof(36)                            // const: worked out while compiling
instr keys(key: num) { osc(tri, mtof(key)) }      // follows the note, gliding if it glides
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

const smooth = [0.25, 0.5, 0.25]
instr soft(freq: hz) { osc(square, freq).fir(smooth) }
```

## 10. Reserved words

Only the words used by this part of the language are listed here. The full list stays in `niminal Spec.md`.

| Kind | Words |
| --- | --- |
| Declarations | `fn`, `const`, `instr`, `mono`, `state`, `with` |
| Control flow | `if`, `else`, `match`, `for`, `in`, `reset_on`, `after` |
| Logic | `and`, `or`, `not`, `true`, `false` |
| Rates | `const`, `init`, `control` |
| Implicit names | `out`, `it`, `note`, `clock` |
| Constants | `pi`, `sample_rate` |
| Units | `num`, `int`, `sec`, `ms`, `samples`, `hz`, `khz`, `db`, `st`, `ct`, `deg`, `bpm`, `beat`, `beats`, `bar`, `bars`, `%` |

`opcode` and `table` are no longer keywords.

## 11. What changed

### From the language before this spec

| Area | Before | Now |
| --- | --- | --- |
| Keywords | `opcode` and `fn` | `fn` only; state makes it a unit generator |
| Untyped parameters | Plain numbers | Generic: any unit, any rate |
| Positional arguments | First only (user); varied (built-ins) | Leading required parameters, for everything |
| Results | Plain numbers only | Any unit; `-> hz` |
| Several results | Body ran once per result | Body runs once |
| Definition order | Opcodes before their callers | Any order; recursion is an error |
| State initial value | A literal | Anything known at note start |
| Ranges | `..` included the end in types, excluded it in lists | `..` includes the end everywhere; `..<` excludes it |
| `clamp`, `range` | Two spellings | A range only |
| Remainder | `%` operator, sensitive to spacing | `x.mod(y)` |
| Logic | Multiply comparisons together | `and`, `or`, `not` |
| `if` | One expression per branch | Blocks; statement form; state writes |
| `for` | Expression only | Also a statement; carries assignments between passes |
| Math | `min`, `max`, `clamp`, `pow` only in opcodes | Everywhere, worked out while compiling where it can be |
| `prev` | One sample in opcodes; bus sends only in instruments | One sample everywhere inside a voice |
| Lists | Compile-time only | Indexing while running, wrapping, `.at`, slices, state lists |
| Tables | `table` keyword (not built) | `const` definitions |
| Rates | Not available | `const`, `init`, `control` on parameters; `.init`, `.control`; work at its own rate in functions with state |
| Choice parameters | Built-ins only (`wave`) | `mode: (a \| b \| c)` for user functions |
| Envelope steps | Literals only | Names and expressions, read at note start |
| `gain` | Reserved; instruments used `amp` | Universal; an instrument may take it over |
| Note context | None | `note.time`, `note.released` |
| Channels to list | `.split` | `.channels` |
| Units after a space | `4 bars` allowed outside brackets | Always attached: `4bars` |
| Voice lifetime | No envelope plus a delay never ended | Ends at release, or after `tail:` |
| Calling an instrument | Not built | Plays its body as part of the voice |

### From the first draft of this spec, while building it

- **Tables are `const` definitions,** not any top-level binding. At the top level a bare bracket is a pattern (`riff = [c4 e4]`), and `[0.25, 0.5]` is a chord there, so a keyword tells the two apart.
- **A state's unit is its starting value's unit,** rather than inferred from what is assigned later: explicit, and it needs no unit inference across lines. `state y = x` covers a generic function.
- **Built-ins expand over arrays** while signals retain channel layout metadata. A stereo signal with `cutoff: [800hz, 1200hz]` pairs each cutoff with its channel. Tables, buffers and computed arrays share fixed-length storage and operations; channel limits apply when converting arrays into audio.
- **`tail:` also applies to voices with envelopes:** a plucked string's envelope is only its excitation.
- **Calling an instrument plays its body in this voice** rather than starting a sub-voice; the envelopes start and release with it.
- **Bus feedback is `late(x)`,** so `prev` is always one sample; `prev` in a send is an error.
- **`schedule` became `after`.** A voice can't start a note through the score model on the audio thread, so a delayed layer inside the voice (`after`), `finish_when` for ending it, and shared buffers (with `note.index`, `note.start`) cover what Csound's `schedule`, `turnoff` and instrument-to-instrument state were used for.
- **`oversample(2|4|8) { ... }`** runs sample DSP with filtered rate conversion and 16 base-rate samples of conversion delay. `sample_rate` and built-in processors use the local rate. Processor scopes nest up to 16 levels; declared processing latency is compensated through parallel paths.
- **`note` is reserved,** like `out` and `it`; the examples use `key: num` for a MIDI note number.

**Implementation note.** Two compilers remain: one builds the block graph for instruments, tracks and functions without state (`lower.rs`), the other the per-sample kernel for functions with state (`kernel.rs`). They share signatures, argument binding, unit rules and the AST walk, and both compile a function for each call, so the rules above hold in both. Merging them into one front end with two back ends is still worth doing if they drift.

## 12. Open questions

1. **Positional arguments.** "Leading required parameters" relaxes the main spec's "positional only for an obvious first argument". It makes built-ins and user functions consistent, and it is shorter live (`sig.lpf(800hz)`).
2. **Bus feedback** is settled: `prev` is always one sample, and `late(x)` is the block-late send between buses.
3. **Functions that build score material.** Brackets in a body are lists, so a `fn` can't write a pattern (`[c2 ~ eb2]`). Score functions (`fn verse(...)` in the main spec) need one. Options include a pattern prefix inside bodies (`p[c2 ~ eb2]`) or letting the expected type decide.
4. **Functions as parameters** are built (§6.7).
5. **`match`** is built (§6.4).
6. **Annotations on bindings** (§3.2). They are left out for now and could be added with the parameter syntax if dogfooding asks for them.
7. **Feedback across several lines in an instrument.** One-line loops work (§5.4); longer loops need a `fn`. Is that the right line to draw?
8. **Checking generic functions where they are defined** is a guess at their units (§5.1). Full unit inference would find more mistakes earlier, at the cost of a more complex checker.

### Instrument structural parameters and subvoices

Instrument choices, functions and explicitly `const` parameters require defaults and specialize the graph. Ordinary numeric arrays require defaults to establish their shape and expose live scalar element paths. Typed `buffer` parameters select shared resources prepared with the voice. Embedded instrument calls can override structural arguments, including tables and untyped const buffer references.

`spawn(trigger, dur: time, max: count) { body }` starts a fresh sample DSP layer on positive edges, capturing inputs and duration. `max` is constant, 1–64, default 8; full pools drop triggers. Each layer has independent note context, state and envelopes, releases at its duration or parent release, then drains. Shared buffers can be read by index or interpolation. Storage is allocated ahead of processing.

Instrument and track `.osc_out` supports `on: trigger`; its 256-event queue preserves repeated values and drops the oldest on overflow. Network output runs outside audio processing.

`array`, `bind_buffer` and `map` events update held notes through the timed, atomic, replayable API. Array values use flat row order. A buffer reference reads through indexing or `.at`; resources must exist when the voice is prepared. Mapping connects controls directly or buses exactly one block late, with an explicit scale and offset in engine units. `source: null` holds the last mapped value. Parameter ranges clamp modulation.

Declared DSP latency propagates through arithmetic, arrays, nested kernels, voices, track effects and feed-forward buses. Faster parallel paths receive sample delays. Musical delays and explicit feedback retain their timing. The engine reports processing latency separately from limiter/device latency. Existing held voices retain the timing of their original graphs across redefinition.
