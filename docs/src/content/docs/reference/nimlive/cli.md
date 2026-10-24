---
title: Command line
description: Every nimlive subcommand and flag — render, live, repl, send, explain, lsp.
---

**Read by nimlive.** This page is the sequencer client: it reads tempo, clips, scenes and patterns, turns them into notes with times, and sends those to the [server](/reference/server/cli/), which makes the sound. It also holds the language server the editor extension uses.

Install it from a clone of the [repo](https://github.com/martineastwood/niminal) (needs a [Rust toolchain](https://rustup.rs/)), next to the server:

```sh
cargo install --path crates/niminal-cli   # niminal, the server
cargo install --path sequencer/nimlive    # nimlive
```

`nimlive <command> --help` lists every flag.

## Subcommands

| Command | What it does |
| --- | --- |
| `nimlive render` | Render a `.nml` file (or an `arrangement` in it) to WAV; optional `--score`, `--from` / `--to`, `--stems` (one file per node and bus into the `--out` folder). |
| `nimlive live [FILE]` | Live sequencer client: clips, scenes, quantized edits, file watch, `--midi`, `--osc-port`. Connects to `--engine-port` (7401); the live-control API on `--port` (7400). |
| `nimlive repl` | REPL connected to a running server (`--port`, `--quantize`, `--token`). |
| `nimlive send` | One-shot eval to the server (code argument, `--file`, or stdin). |
| `nimlive explain FILE` | Compile a `.nml` file and list, for each instrument and node chain, the ops that run every sample and the ones that run once per block (see [control scopes](/reference/language/units-and-rates/#control-scopes)). |
| `nimlive lsp` | Language server on stdio — see [Editor and LSP](/reference/editor/). |

## Render

`--out` is required. Live pieces without a fixed end use `--bars` or `--seconds`. Offline renders can use `--no-limiter`. `nimlive render file.nml --score events.nms --out out.wav` plays a [score](/reference/score-model/) against the file's instruments, and `--input take.wav` plays a WAV file into the file's fx, for as long as the file lasts unless you say otherwise with `--bars` or `--seconds`.

Pass the same `--block-size` and `--control-step` the server runs with when you want a render to match what you hear — both are fixed at server start-up and not recorded in session logs.

## Live, send and the REPL

`nimlive live [FILE]` loads a project and reloads it on save. Add `--midi` and/or `--osc-port` to play from a keyboard or another program; see `examples/midi_osc.nml`.

`--quantize` accepts values like `now`, `next bar`, `next 4bars` (same as in source `@ next 4bars`), and works with `nimlive send` and in the REPL. In the REPL you can also set or clear the default with `:quantize WHEN` or `:quantize off`, and leave with `:quit`.

## Server flags

Channels, devices, logging, session logs and the rest belong to the server: see [`niminal`'s command line](/reference/server/cli/).
