---
title: Command line
description: The niminal subcommands.
---

There are two programs. `niminal` is the sound server. `nimlive` is a live-coding sequencer that is one client of it; it owns clips, scenes, patterns and tempo. Install from source (needs a [Rust toolchain](https://rustup.rs/)):

```sh
cargo install --path crates/niminal-cli   # niminal
cargo install --path sequencer/nimlive    # nimlive
```

Run `niminal <command> --help` or `nimlive <command> --help` for every flag.

## Subcommands

| Command | What it does |
| --- | --- |
| `niminal daemon` | Generic sound server and [events API](/reference/engine-api/) on port 7401 (`--port`), with audio, `--log`, `--token`, `--no-audio`. |
| `niminal replay` | Render a session log (what a daemon recorded with `--log`, or any client wrote) to WAV. |
| `nimlive render` | Render a `.nml` file (or an `arrangement` in it) to WAV; optional `--score`, `--from` / `--to`, `--stems` (one file per track and bus into the `--out` folder). |
| `nimlive live [FILE]` | Separate live sequencer client: clips, scenes, quantized edits, file watch, `--midi`, `--osc-port`. Connects to `--engine-port` (7401); live-control API on `--port` (7400). |
| `nimlive repl` | REPL connected to a running daemon (`--port`, `--quantize`, `--token`). |
| `nimlive send` | One-shot eval to the daemon (code argument, `--file`, or stdin). |
| `nimlive lsp` | Language server on stdio — see [Editor and LSP](/reference/editor/). |
| `niminal docs` | Emit built-in opcode reference as Markdown (used when building the docs site). |

## Common flags

**Daemon** — default listen `127.0.0.1:7401`, stereo output. `--channels` supports 1, 2, 4, 6 (5.1), 8 (7.1). `nimlive live [FILE]` loads a project and reloads it on save. Session logs default to `~/.niminal/sessions/` (newest 20 kept) unless `--no-log` or `--log PATH`. Crashes append to `~/.niminal/crash.log`.

**Render** (`nimlive`) — `--out` is required. Live pieces without a fixed end use `--bars` or `--seconds`. Offline renders can use `--no-limiter`. Replay: `niminal replay <log> --out set.wav --channels 2`. Score: `nimlive render file.nml --score events.nms --out out.wav`.

**Send / REPL** — `--quantize` accepts values like `now`, `next bar`, `next 4bars` (same as in source `@ next 4bars`). In the REPL you can also set or clear the default with `:quantize WHEN` or `:quantize off`, and leave with `:quit`.
