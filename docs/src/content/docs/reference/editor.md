---
title: Editor and LSP
description: VS Code extension, language server, and editing without a server.
---

## Language server (`nimlive lsp`)

The CLI runs an **LSP server on stdio**. Any editor that can spawn a language server can use it for `.nml` files:

```sh
nimlive lsp
```

The VS Code extension starts the same binary automatically (see settings below).

**Works without a running server** — editing support comes from the compiler in-process:

- Diagnostics as you type (compiler messages and help)
- **Completion** for names in the file, built-in opcodes, methods after `.`, named arguments (`lpf(` → `cutoff:`, `res:`), waveforms, unit suffixes
- **Hover** with signatures and documentation
- **Go to definition** and **document outline**
- A `//` or `///` comment directly above an `instr` or `fn` is its documentation

Trigger character for completion: `.`

After installing a new `niminal` binary, run **Restart Language Server** in the command palette (VS Code) or restart your editor’s LSP client.

## VS Code / Cursor extension

From the repo root:

```sh
cd editors/vscode && npm install && node install.js
```

Reload the window (`--cursor` for Cursor). Build or install the CLI first (`cargo install --path crates/niminal-cli`).

Start a server in a terminal, or use **niminal: Start Server in a Terminal** from the command palette:

```sh
niminal server
```

Open a `.nml` file. Live evaluation talks to the server over WebSocket; the language server is separate (stdio).

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
| `niminal.path` | Path to the `niminal` binary (server + LSP) |
| `niminal.port` | Server WebSocket port (default `7401`) |
| `niminal.token` | Token if the server was started with `--token` |
| `niminal.quantize` | Default land time for evaluation (e.g. `next 4bars`) |

### MIDI learn

With the live client started with `--midi`, a control written as `midi.learn` waits for the next CC. When one arrives, the extension can rewrite the line to the bound `midi.cc(...)`. See [Performance](/reference/language/performance/) and [Live coding](/guide/live-coding/).

## Server vs LSP

| | **LSP** | **Server** |
| --- | --- | --- |
| Transport | stdio | Private live client → generic server (7401) |
| Audio | No | Yes |
| Live `start` / `stop` / changes | No | Yes |
| Diagnostics / completion | Yes | Eval diagnostics via extension/REPL |

For command-line workflow (REPL, `send`, render, replay), see [Command line](/reference/cli/) and [Live coding](/guide/live-coding/). For WebSocket messages, subscriptions, and `eval` / `events.send`, see [Server protocol](/reference/protocol/).
