---
title: Command line
description: Every niminal-live subcommand and flag — render, gallery, live, repl, send, explain, lsp.
---

**Read by niminal-live.** This page is the sequencer client: it reads tempo, clips, scenes and patterns, turns them into notes with times, and sends those to the [server](/reference/server/cli/), which makes the sound. It also holds the language server the editor extension uses.

Install it from a clone of the [repo](https://github.com/martineastwood/niminal) (needs a [Rust toolchain](https://rustup.rs/)), next to the server:

```sh
cargo install --path crates/niminal-cli   # niminal, the server
cargo install --path sequencer/live-cli    # niminal-live
```

`niminal-live <command> --help` lists every flag.

## Subcommands

| Command | What it does |
| --- | --- |
| `niminal-live render` | Render a `.nml` file (or an `arrangement` in it) to WAV; optional `--score`, `--from` / `--to`, `--stems` (one file per node and bus into the `--out` folder). |
| `niminal-live gallery SAMPLE` | Make many versions of a sample, each a short piece of code, and render them to listen to. See [Gallery](#gallery). |
| `niminal-live live [FILE]` | Live sequencer client: clips, scenes, quantized edits, file watch, `--midi`, `--osc-port`, `--clock-in`, `--clock-out`. Connects to `--engine-port` (7401); the live-control API on `--port` (7400). |
| `niminal-live repl` | REPL connected to a running server (`--port`, `--quantize`, `--token`). |
| `niminal-live send` | One-shot eval to the server (code argument, `--file`, or stdin). |
| `niminal-live explain FILE` | Compile a `.nml` file and list, for each instrument and node chain, the ops that run every sample and the ones that run once per block (see [control scopes](/reference/language/units-and-rates/#control-scopes)). |
| `niminal-live lsp` | Language server on stdio — see [Editor and LSP](/reference/editor/). |

## Render

`--out` is required. Live pieces without a fixed end use `--bars` or `--seconds`. Offline renders can use `--no-limiter`. `niminal-live render file.nml --score events.nms --out out.wav` plays a [score](/reference/score-model/) against the file's instruments, and `--input take.wav` plays a WAV file into the file's fx, for as long as the file lasts unless you say otherwise with `--bars` or `--seconds`.

Pass the same `--block-size` and `--control-step` the server runs with when you want a render to match what you hear — both are fixed at server start-up and not recorded in session logs.

## Gallery

```sh
niminal-live gallery break.wav --out gallery/ --families players,manglers --amount enough
```

Writes a `.nml` and a rendered `.wav` for each version into `--out`, and a `gallery.json` that lists them with the settings each was made from. Every version is ordinary code you can open, edit and keep: a recipe with its settings picked at random inside their ranges.

| Flag | Meaning |
| --- | --- |
| `--families` | Only these kinds of recipe, comma separated: `players` (grains and clouds of the sample), `manglers` (fragments, reversed debris, held spectra), `glitch` (stutters, rewinds, tape stops, shuffles and crushes of the sample, in time with the tempo), `synthesis` (the sample played as a waveform), `modifiers` (pitch shifts and filter sweeps) and `replicators` (echo trains). All of them without it. |
| `--amount` | `few` (1 version of each recipe), `enough` (3, the default) or `lots` (6). |
| `--seed` | The same seed makes the same versions. Change it for a new set. |

A version that fails to render is reported and skipped; the rest carry on. The recipes are the files in `sequencer/live-cli/gallery/`: a comment header gives the family and each setting's range (`// density: 8..120 log`, with `int` for whole numbers), and `{density}`, `{seed}` and `{sample}` are filled in.

## Drums

`niminal-live drums` prints a drum `grid { ... }` made from a theme, as code to paste into a piece and edit. The same choices always make the same grid.

```sh
niminal-live drums --theme house --density 0.8 --chaos 0.3 --seed 4 --drum hat=0.6 --name beat
```

- `--theme`: `techno`, `house`, `two_step`, `halftime`, `breaks`, `boom_bap`, `trap` or `dnb`.
- `--density`: how busy it is, 0 to 1. Thinning takes the ghost notes out first and the anchors of the groove last.
- `--chaos`: how far it wanders from the theme, 0 to 1. With 0 the seed makes no difference.
- `--drum hat=0.6`: make one drum sparser (below 1) or busier (above 1) than the rest. Repeat it for more drums.
- `--seed`: another number makes another wander.
- `--name beat`: writes `beat = grid { ... }`.

The rows are named `kick`, `snare` or `clap`, `hat`, `open` and `perc`, depending on the theme. Map them to instruments in a `kit`, as `examples/generated_drums.nml` does.

## Live, send and the REPL

`niminal-live live [FILE]` loads a project and reloads it on save. Add `--midi` and/or `--osc-port` to play from a keyboard or another program; see `examples/midi_osc.nml`. `--clock-in NAME` follows another device's MIDI clock and `--clock-out NAME` sends one; see [Live coding](/guide/live-coding/).

`--quantize` accepts values like `now`, `next bar`, `next 4bars` (same as in source `@ next 4bars`), and works with `niminal-live send` and in the REPL. In the REPL you can also set or clear the default with `:quantize WHEN` or `:quantize off`, and leave with `:quit`.

## Server flags

Channels, devices, logging, session logs and the rest belong to the server: see [`niminal`'s command line](/reference/server/cli/).
