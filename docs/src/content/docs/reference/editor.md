---
title: Editor and LSP
description: VS Code extension, language server, and editing without a server.
---

**Read by Niminal Stage.** The extension is an editor for Niminal Stage: it runs the `niminal` language server, and the code you evaluate plays through Stage.

## Language server (`niminal lsp`)

`niminal lsp` runs an **LSP server on stdio**. Any editor that can spawn a language server can use it for `.nml` files:

```sh
niminal lsp
```

The macOS download puts `niminal` in `/Applications/Niminal/bin`. Stage need not be running.

The VS Code extension starts it automatically, finding `niminal` in the download's `bin` folder, on your PATH or in `~/.cargo/bin` (see settings below).

**Works without a running server** — editing support comes from the compiler in-process:

- Diagnostics as you type (compiler messages and help)
- **Completion** for names in the file, built-in opcodes, methods after `.`, named arguments (`lpf(` → `cutoff:`, `res:`), waveforms, unit suffixes
- **Hover** with signatures and documentation
- **Rate hints** (LSP inlay hints) after each named value in an instrument body that changes at note start (`· init`), once per block (`· control`) or every sample (`· audio`), so you can see where a `control { ... }` scope would save work. Constants have none. Hints appear while the file compiles.
- **Go to definition** and **document outline**
- A `//` or `///` comment directly above an `instr` or `fn` is its documentation

Trigger character for completion: `.`

After installing a new `niminal` binary, run **Restart Language Server** in the command palette (VS Code) or restart your editor’s LSP client.

## VS Code / Cursor extension

With the macOS download, choose **File > Install VS Code Extension…** in Stage (it also offers it the first time it opens). From the repo root:

```sh
cd editors/vscode && npm install && node install.js
```

Reload the window (`--cursor` for Cursor). The language server comes from the Niminal Stage app, so install that first.

The code you evaluate plays through Niminal Stage, which has the engine and the sequencer in it. If Stage is not running, the first evaluation offers to open it (`niminal.stageApp` names the app). Stage serves the sequencer on its tools port (7400 by default), and the extension joins its `main` chair: what you evaluate in the editor shows in Stage's lanes and scene buttons and plays on its transport. If both define the same name, the last one evaluated wins.

The **niminal** sidebar lists scenes and clips. A scene shows how many of its clips are playing, and starts, mutes, solos and stops them as a group; a clip shows what has been done to it and can be muted, soloed and stopped. The file's scenes launch with Cmd/Ctrl+1..9. The status bar flags a soloed or muted clip (`SOLO bass · 2 muted`), since either silences something that is playing. Compile errors show as squiggles at the right place and in the **niminal** output channel; the status bar shows bar, beat, tempo, voices and pending changes, and clicking it hushes. **niminal: Connect to Server** and **niminal: Panic** are in the command palette. Evaluating starts the window's live sequencer; if the server isn't answering, the extension offers to start it in a terminal.

### Settings

| Setting | Purpose |
| --- | --- |
| `niminal.path` | The `niminal` command, which runs the language server (`niminal lsp`). Found in the download's `bin` folder or on your PATH if you leave it |
| `niminal.stageApp` | The Stage app to open when it is not running (macOS; default `Niminal Stage`) |
| `niminal.port` | The tools port Stage serves the sequencer on (default `7400`) |
| `niminal.token` | The token Stage asks for, if one is set |
| `niminal.quantize` | Default land time for evaluation (e.g. `next 4bars`); empty uses the sequencer's default |

### MIDI learn

With a MIDI input chosen in Stage, a control written as `midi.learn` waits for the next CC. When one arrives, the extension rewrites the line to the bound `midi.cc(...)`. See [Performance](/reference/language/performance/) and [Live coding](/guide/live-coding/).

## Live session vs LSP

| | **LSP** | **Live session** |
| --- | --- | --- |
| Transport | stdio | Private live client → generic server (7401) |
| Audio | No | Yes |
| Live `start` / `stop` / changes | No | Yes |
| Diagnostics / completion | Yes | Eval diagnostics via extension/REPL |

For the command line (`niminal play`, render, replay), see [Command line](/reference/server/cli/) and [Live coding](/guide/live-coding/). For WebSocket messages, subscriptions, and `eval` / `events.send`, see [Sequencer protocol](/reference/protocol/).
