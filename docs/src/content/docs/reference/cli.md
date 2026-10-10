---
title: Command line
description: The niminal subcommands.
---

Install from source:

```sh
cargo install --path crates/niminal-cli
```

Run `niminal <command> --help` for every flag.

## Subcommands

| Command | What it does |
| --- | --- |
| `niminal render` | Render a `.nml` file (or an `arrangement` in it) to WAV; optional `--score`, `--from` / `--to`, stems-style workflows via arrangement; or `--replay` a session log. |
| `niminal daemon` | Live engine: audio, [WebSocket protocol](/reference/protocol/), optional `--midi`, `--osc-port`, `--log`, `--token`, `--no-audio`. |
| `niminal repl` | REPL connected to a running daemon (`--port`, `--quantize`, `--token`). |
| `niminal send` | One-shot eval to the daemon (code argument, `--file`, or stdin). |
| `niminal lsp` | Language server on stdio — see [Editor and LSP](/reference/editor/). |
| `niminal docs` | Emit built-in opcode reference as Markdown (used when building the docs site). |

## Common flags

**Daemon** — default listen `127.0.0.1:7400`, stereo output. `--channels` supports 1, 2, 4, 6 (5.1), 8 (7.1). Session logs default to `~/.niminal/sessions/` unless `--no-log` or `--log PATH`.

**Render** — `--out` is required. Live pieces without a fixed end use `--bars` or `--seconds`. Offline renders can use `--no-limiter`. Replay: `niminal render --replay <log> --out set.wav --channels 2`.

**Send / REPL** — `--quantize` accepts values like `now`, `next bar`, `next 4 bars` (same as in source `@ next 4 bars`).
