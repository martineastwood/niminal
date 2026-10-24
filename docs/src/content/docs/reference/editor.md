---
title: Editor and LSP
description: VS Code extension, language server, and editing without a server.
---

**Read by nimlive.** The extension is an editor around nimlive: it runs the language server, and a private live sequencer, for you.

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
- **Rate hints** (LSP inlay hints) after each named value in an instrument body that changes at note start (`· init`), once per block (`· control`) or every sample (`· audio`), so you can see where a `control { ... }` scope would save work. Constants have none. Hints appear while the file compiles.
- **Go to definition** and **document outline**
- A `//` or `///` comment directly above an `instr` or `fn` is its documentation

Trigger character for completion: `.`

After installing a new `nimlive` binary, run **Restart Language Server** in the command palette (VS Code) or restart your editor’s LSP client.

## VS Code / Cursor extension

From the repo root:

```sh
cd editors/vscode && npm install && node install.js
```

Reload the window (`--cursor` for Cursor). Build or install both binaries first:

```sh
cargo install --path crates/niminal-cli   # niminal, the server
cargo install --path sequencer/nimlive    # nimlive, the live sequencer and language server
```

Start a server in a terminal, or use **niminal: Start Server in a Terminal** from the command palette:

```sh
niminal server
```

Open a `.nml` file. The first evaluation starts a private `nimlive live --port 0 --parent-stdio` process owned by that window: clips, scenes, lanes and quantized edits live there, and only generic events reach the server. Each window has its own performance state, and closing it stops the sequencer. The language server is separate (stdio).

### Shortcuts

| Key (mac / other) | Action |
| --- | --- |
| Cmd+Enter / Ctrl+Enter | Evaluate selection, or the block around the cursor (blank-line blocks, bracket-balanced) |
| Cmd+Shift+Enter / Ctrl+Shift+Enter | Evaluate whole file |
| Cmd+. / Ctrl+. | Hush: stop every clip, letting what's sounding ring out |
| Cmd+1..9 | Launch the Nth scene from the sidebar (while a `.nml` file has focus) |

The **niminal** sidebar lists scenes and clips. A scene shows how many of its clips are playing, and starts, mutes, solos and stops them as a group; a clip shows what has been done to it and can be muted, soloed and stopped. The file's scenes launch with Cmd/Ctrl+1..9. The status bar flags a soloed or muted clip (`SOLO bass · 2 muted`), since either silences something that is playing. Compile errors show as squiggles at the right place and in the **niminal** output channel; the status bar shows bar, beat, tempo, voices and pending changes, and clicking it hushes. **niminal: Connect to Server** and **niminal: Panic** are in the command palette. Evaluating starts the window's live sequencer; if the server isn't answering, the extension offers to start it in a terminal.

### Settings

| Setting | Purpose |
| --- | --- |
| `niminal.path` | Path to the `niminal` binary (the audio server) |
| `niminal.livePath` | Path to the `nimlive` binary (live sequencer + LSP) |
| `niminal.port` | Server WebSocket port (default `7401`) |
| `niminal.token` | Token if the server was started with `--token` |
| `niminal.quantize` | Default land time for evaluation (e.g. `next 4bars`); empty uses the sequencer's default |
| `niminal.midi` | MIDI input name filter for the window's sequencer: empty string listens to all inputs, `null` disables MIDI. Reload the editor after changing |
| `niminal.oscPort` | OSC input UDP port for the window's sequencer (`null` disables OSC). Reload the editor after changing |

### MIDI learn

With MIDI enabled (`niminal.midi`), a control written as `midi.learn` waits for the next CC. When one arrives, the extension rewrites the line to the bound `midi.cc(...)`. See [Performance](/reference/language/performance/) and [Live coding](/guide/live-coding/).

## Live session vs LSP

| | **LSP** | **Live session** |
| --- | --- | --- |
| Transport | stdio | Private live client → generic server (7401) |
| Audio | No | Yes |
| Live `start` / `stop` / changes | No | Yes |
| Diagnostics / completion | Yes | Eval diagnostics via extension/REPL |

For command-line workflow (REPL, `send`, render, replay), see [Command line](/reference/nimlive/cli/) and [Live coding](/guide/live-coding/). For WebSocket messages, subscriptions, and `eval` / `events.send`, see [Sequencer protocol](/reference/protocol/).
