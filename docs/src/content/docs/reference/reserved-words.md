---
title: Reserved words
description: Names that cannot be used as identifiers in niminal.
---

**Read by both programs.** These are names the whole language keeps, whichever program reads the file.

**Never a name.** None of these can be the name of anything you define: a constant, function, instrument, clip, scene, parameter, local variable or loop variable. The error says "`name` is a reserved word".

| Kind | Names |
| --- | --- |
| Control flow | `if`, `else`, `match`, `for`, `in`, `after`, `spawn`, `oversample`, `reset_on` |
| Logic and rates | `and`, `or`, `not`, `true`, `false`, `init`, `control`, `state`, `const`, `fn` |
| Names the language provides | `out`, `it`, `note`, `clock`, `instrument`, `env`, `grid`, `pi`, `sample_rate` |

**Not the name of a top-level definition.** These begin a declaration or a line at the top level of a file, so a constant, instrument, clip, scene, bus or the like can't be called one. Inside an instrument, function or effect they are free, so `instr voice(sample)` and a local called `at` or `capture` are fine.

| Kind | Names |
| --- | --- |
| Declarations | `instr`, `mono`, `def`, `bus`, `buffer`, `sample`, `kit`, `sfz`, `ctl`, `macro`, `node`, `fx`, `clip`, `scene`, `arrangement`, `config`, `use`, `as`, `midi_out`, `events` |
| Statements that begin a line | `tempo`, `meter`, `at`, `on`, `hush`, `panic`, `reset`, `new`, `unsolo`, `capture` |

Words that only mean something after a dot or an `@` are free to use as names: `start`, `stop`, `mute`, `unmute`, `solo`, `play`, `undo`, `clear`, `set`, `last`, `next`, `now`, `over` and `cycle` (so `clip next { ... }` and `next.start` work). So are the unit words (`hz`, `bar`, `beat`, ...) and the universal controls (`pan`, `to`, `dur`, `nudge`), with two exceptions: a unit word can't be the name of a *function* (below), and an instrument's own `gain` parameter must be in db, as every note's is.

## Built-in functions

A function you write with `fn` can't have the name of a built-in either, since `x.name()` would mean two things. That covers every opcode in the [opcode reference](/reference/opcodes/), the math functions (`sin`, `cos`, `tanh`, `exp`, `abs`, `sqrt`, ...), and these:

| Kind | Names |
| --- | --- |
| Arrays and signals | `map`, `sum`, `len`, `channels`, `indices`, `at`, `phase`, `step`, `active`, `normalize`, `reverse`, `rotate`, `resample`, `concat`, `integrate`, `mix`, `record`, `data` |
| Numbers | `min`, `max`, `pow`, `mod`, `atan2`, `clamp`, `hold`, `smooth`, `init`, `control`, `late`, `prev`, `range`, `zeros` |
| Vectors and frames | `dot`, `fir`, `taps`, `matvec`, `frames`, `fft`, `ifft` |
| Tables | `harmonics`, `partials`, `cosines`, `curve`, `steps`, `poly`, `ramp`, `cheby`, `window`, `random`, `file`, `bank` |
| Scopes and taps | `reset_on`, `spawn`, `oversample`, `after`, `report`, `osc_out`, `input` |

The unit words above (`hz`, `bar`, `st`, ...) are reserved as function names too, because `x.bar` turns a number into bars. The error says which it is: "`bar` is a unit, so it can't be the name of a function", or "`dot` is a built-in function, so it can't be defined again".

