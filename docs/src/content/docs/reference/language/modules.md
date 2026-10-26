---
title: Modules
description: Split a project across files with use.
---

**Read by both programs.** The server and niminal-live each follow `use` when they read a file.

A module is a file. `use rig` loads `rig.nml` and adds its definitions to yours.

```niminal
use voices
use score/verse
use acme_verbs as av

pluck(freq: 440hz) for 1beat
out = sig.av::shimmer(size: 0.9)
```

## Where modules are found

`use std/filters` and the other `std/` modules come with niminal and are read from the program itself: see the [standard library](/reference/language/standard-library/).

In order: next to the file that uses it, a `lib` folder under the project folder, then the installed packages. `use score/verse` looks for `score/verse.nml`.

Installed packages are the `.nml` files in `$NIMINAL_PACKAGES`, or in `~/.niminal/packages` when that is not set. To install one, put it there.

## Names

- `use rig` brings in everything `rig` defines, under the names it gave them.
- `use rig as r` brings them in only as `r::name`, so two modules can define the same name. Write `r::shimmer`, also after a dot: `sig.r::shimmer()`.
- A name that starts with `_` is private to its module. Another file can't use it, and it never clashes with a name of yours.
- A module loads once, however many files use it, and its definitions come before the file that uses it. Use a module under one name only: `use rig` and `use rig as r` together is an error.
- A module's nodes, patterns and `play` lines start with it.
- A module that uses itself, directly or through another module, is an error.

## Errors and reloading

- An error in a module is reported on the `use` line, with the module's own file and line.
- `niminal-live live file.nml` watches every module the file uses and evaluates it again, as a whole, when any of them changes.
- The evaluation log keeps the joined code, so a log replays without the module files.
- The language server completes, hovers and jumps to names from modules. After `r::` it offers what `r` defines.

## Limits

An alias is one name for the whole project, so two files can't use different modules as the same alias. Names inside a pattern's brackets aren't qualified, which only matters for kit samples.
