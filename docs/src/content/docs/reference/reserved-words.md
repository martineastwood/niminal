---
title: Reserved words
description: Names that cannot be used as identifiers in niminal.
---

None of these can be used as a name.

| Kind | Names |
| --- | --- |
| Declarations | `instr`, `mono`, `opcode`, `fn`, `state`, `bus`, `buffer`, `table`, `sample`, `kit`, `ctl`, `track`, `clip`, `scene`, `arrangement`, `config`, `use`, `as`, `with` |
| Performance and score | `play`, `stop`, `mute`, `unmute`, `solo`, `unsolo`, `launch`, `hush`, `panic`, `freeze`, `unfreeze`, `tempo`, `meter`, `at`, `for`, `over`, `next`, `now`, `cycle` |
| Control flow | `if`, `else`, `for`, `in` |
| Literal prefixes | `env[...]`, `grid[...]`, `grid { ... }`; kit name before brackets (`drums[...]`) |
| Units | `sec`, `ms`, `samples`, `hz`, `khz`, `db`, `st`, `deg`, `bpm`, `beat`, `beats`, `bar`, `bars`, `%` |
| Implicit names | `out`, `it` |
| Universal controls | `pan`, `gain`, `to`, `dur`, `nudge` |
