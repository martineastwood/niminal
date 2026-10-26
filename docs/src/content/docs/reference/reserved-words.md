---
title: Reserved words
description: Names that cannot be used as identifiers in niminal.
---

**Read by both programs.** These are names the whole language keeps, whichever program reads the file.

None of these can be used as a name.

| Kind | Names |
| --- | --- |
| Declarations | `instr`, `mono`, `fn`, `const`, `state`, `bus`, `buffer`, `sample`, `kit`, `ctl`, `node`, `fx`, `clip`, `scene`, `arrangement`, `config`, `use`, `as` |
| Performance and score | `start`, `stop`, `play`, `mute`, `unmute`, `solo`, `unsolo`, `undo`, `clear`, `set`, `hush`, `panic`, `reset`, `new`, `capture`, `last`, `tempo`, `meter`, `at`, `for`, `over`, `next`, `now`, `on`, `cycle` |
| Control flow | `if`, `else`, `match`, `for`, `in`, `reset_on`, `after`, `spawn`, `oversample` |
| Logic | `and`, `or`, `not`, `true`, `false` |
| Rates | `const`, `init`, `control` |
| Literal prefixes | `env[...]`, `grid[...]`, `grid { ... }` |
| Units | `num`, `int`, `sec`, `ms`, `samples`, `hz`, `khz`, `db`, `st`, `ct`, `deg`, `bpm`, `beat`, `beats`, `bar`, `bars`, `%` |
| Implicit names | `out`, `in`, `it`, `note`, `clock` |
| Constants | `pi`, `sample_rate` |
| Universal controls | `pan`, `gain`, `to`, `dur`, `nudge` |

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

