---
title: Command line
description: The niminal subcommands.
---

Install from source (needs a [Rust toolchain](https://rustup.rs/)):

```sh
cargo install --path crates/niminal-cli
# or: cargo install --git https://github.com/martineastwood/niminal niminal-cli
```

Run `niminal <command> --help` for every flag.

## Subcommands

| Command | What it does |
| --- | --- |
| `niminal render` | Render a `.nml` file (or an `arrangement` in it) to WAV; optional `--score`, `--from` / `--to`, `--stems` (one file per track and bus into the `--out` folder); or `--replay` a session log. |
| `niminal daemon` | Generic sound server and [events API](/reference/engine-api/) on port 7401 (`--port`), with audio, `--log`, `--token`, `--no-audio`. |
| `niminal live [FILE]` | Separate live sequencer client: clips, scenes, quantized edits, file watch, `--midi`, `--osc-port`. Connects to `--engine-port` (7401); live-control API on `--port` (7400). |
| `niminal repl` | REPL connected to a running daemon (`--port`, `--quantize`, `--token`). |
| `niminal send` | One-shot eval to the daemon (code argument, `--file`, or stdin). |
| `niminal lsp` | Language server on stdio — see [Editor and LSP](/reference/editor/). |
| `niminal docs` | Emit built-in opcode reference as Markdown (used when building the docs site). |

## Common flags

**Daemon** — default listen `127.0.0.1:7401`, stereo output. `--channels` supports 1, 2, 4, 6 (5.1), 8 (7.1). `niminal live [FILE]` loads a project and reloads it on save. Session logs default to `~/.niminal/sessions/` (newest 20 kept) unless `--no-log` or `--log PATH`. Crashes append to `~/.niminal/crash.log`.

**Render** — `--out` is required. Live pieces without a fixed end use `--bars` or `--seconds`. Offline renders can use `--no-limiter`. Replay: `niminal render --replay <log> --out set.wav --channels 2`. Score: `niminal render file.nml --score events.nms --out out.wav`.

**Send / REPL** — `--quantize` accepts values like `now`, `next bar`, `next 4bars` (same as in source `@ next 4bars`). In the REPL you can also set or clear the default with `:quantize WHEN` or `:quantize off`, and leave with `:quit`.
