---
title: Editor and LSP
description: VS Code extension, language server, and editing without a daemon.
---

## Language server (`niminal lsp`)

The CLI runs an **LSP server on stdio**. Any editor that can spawn a language server can use it for `.nml` files:

```sh
niminal lsp
```

The VS Code extension starts the same binary automatically (see settings below).

**Works without a running daemon** — editing support comes from the compiler in-process:

- Diagnostics as you type (compiler messages and help)
- **Completion** for names in the file, built-in opcodes, methods after `.`, named arguments (`lpf(` → `cutoff:`, `res:`), waveforms, unit suffixes
- **Hover** with signatures and documentation
- **Go to definition** and **document outline**
- A `//` or `///` comment directly above an `instr` or `opcode` is its documentation

Trigger character for completion: `.`

After installing a new `niminal` binary, run **Restart Language Server** in the command palette (VS Code) or restart your editor’s LSP client.

## VS Code extension

From the repo root:

```sh
cd editors/vscode && npm install && node install.js
```

Reload the window (`--cursor` for Cursor). Build or install the CLI first (`cargo install --path crates/niminal-cli`).

Start a daemon in a terminal, or use **niminal: Start Daemon in a Terminal** from the command palette:

```sh
niminal daemon
```

Open a `.nml` file. Live evaluation talks to the daemon over WebSocket; the language server is separate (stdio).

### Shortcuts

| Key (mac / other) | Action |
| --- | --- |
| Cmd+Enter / Ctrl+Enter | Evaluate selection, or the block around the cursor (blank-line blocks, bracket-balanced) |
| Cmd+Shift+Enter / Ctrl+Shift+Enter | Evaluate whole file |
| Cmd+. / Ctrl+. | Hush |
| Cmd+1..9 | Launch the Nth scene from the sidebar (while a `.nml` file has focus) |

The **niminal** sidebar lists scenes and tracks (mute, solo, stop). Compile errors show as squiggles; the status bar shows bar, beat, tempo, voices, and pending quantize. **niminal: Panic** is in the command palette.

### Settings

| Setting | Purpose |
| --- | --- |
| `niminal.path` | Path to the `niminal` binary (daemon + LSP) |
| `niminal.port` | Daemon WebSocket port (default `7400`) |
| `niminal.token` | Token if the daemon was started with `--token` |
| `niminal.quantize` | Default land time for evaluation (e.g. `next 4 bars`) |

## Daemon vs LSP

| | **LSP** | **Daemon** |
| --- | --- | --- |
| Transport | stdio | WebSocket (default port 7400) |
| Audio | No | Yes |
| Live `play` / `launch` | No | Yes |
| Diagnostics / completion | Yes | Eval diagnostics via extension/REPL |

For command-line workflow (REPL, `send`, render, replay), see [Command line](/reference/cli/) and [Live coding](/guide/live-coding/). For WebSocket messages, subscriptions, and `eval` / `control.set`, see [Daemon protocol](/reference/protocol/).
