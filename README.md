# niminal

**Alpha.** APIs, syntax, and behavior change without notice. Expect rough edges, missing pieces, and the occasional crash. Not ready for production or long-term projects yet.

niminal is an audio server with its own DSP language. It compiles instruments, effects and routing, and plays timestamped events sent to it by any client. It knows nothing of clips, scenes, patterns or tempo.

`niminal-live` is the first client: a live-coding sequencer that adds clips, scenes, mini-notation patterns, tempo and quantized changes on top, and a VS Code extension to drive it. The [tracker](tracker/) is a second client of the same API.

```
crates/      the audio server: engine, DSP language, WebSocket API, `niminal` CLI
sequencer/   a client: the composition language, patterns, `niminal-live`
editors/     the VS Code extension, a client of `niminal-live`
clients/     JS and Python SDKs for the server's API
```

The server's crates never depend on `sequencer/`; it is a separate Cargo workspace that depends on them.

[Documentation](https://niminal.dev) · [Language reference](https://niminal.dev/reference/language/) · [GitHub](https://github.com/martineastwood/niminal)

## Install

You need a recent [Rust toolchain](https://rustup.rs/).

From a clone of this repo, install the audio server and the live sequencer:

```sh
cargo install --path crates/niminal-cli      # niminal
cargo install --path sequencer/live-cli       # niminal-live
```

## Hear something quickly

The repo includes examples under `examples/`. Render one to a WAV file:

```sh
niminal-live render examples/saw_lead.nml --out saw_lead.wav
```

Try [spectral_delay.nml](examples/spectral_delay.nml) for plucks with frequency-dependent echoes:
it compares ordinary echoes with spread-out harmonics, then plays a short phrase.

Open `saw_lead.nml` to see the language: tempo, instruments with units (`hz`, `db`, `ms`), envelopes, and scored notes.

## Live coding

Start the audio server, then the live sequencer client:

```sh
niminal server
# in another terminal:
niminal-live live examples/groove.nml
niminal-live repl
# in another terminal:
niminal-live send 'chorus.start' --quantize "next 4bars"
```

Sessions are logged sample-accurately, so you can replay them:

```sh
niminal replay ~/.niminal/sessions/<log> --out set.wav
```

For MIDI / OSC controls, start the live client with `--midi` and/or `--osc-port` (see `examples/midi_osc.nml`). Use `--help` for flags (`niminal`: `server`, `replay`, `docs`; `niminal-live`: `render`, `live`, `repl`, `send`, `lsp`).

## Editor

The [VS Code / Cursor extension](editors/vscode/) adds syntax highlighting, diagnostics, and evaluate-while-playing against a running server. Install it from `editors/vscode` (see that folder’s README), run `niminal server`, the extension starts its own private live sequencer, then evaluate with **Cmd+Enter** (macOS) or **Ctrl+Enter**.

The language server (`niminal-live lsp`) works without a server for hover, completion, and errors.

## Stage

[`stage/`](stage/) is a Tauri desktop app for live coding: the editor beside scenes, lanes that light up as clips play, meters, and a controls dock. It has the sequencer built in, so it needs only `niminal server`. See the [Stage page](https://niminal.dev/reference/stage/).

## Tracker

[`tracker/`](tracker/) is an independent Tauri desktop app with a keyboard-driven pattern grid and a CodeMirror instrument/effect editor using the niminal LSP. It supports looping playback, auditioning, a track mixer with effects, pattern selection/copy/paste/undo, and project save/load. See the [tracker manual](https://niminal.dev/reference/tracker/) for how to use it, and its [README](tracker/README.md) for build commands.

## What you get

- **Live** — WebSocket server, REPL, `send`, and editor integration; quantize changes to musical time. `niminal server` is the audio server (the engine), with the live sequencer as one client of its event API, so other programs can play through it too. Clients own tempo, meter and all musical scheduling.
- **Frontend API** — Browser, Node and Python clients can own private fx definitions and instances, route and modulate them, stream WAV files, inspect voice lifecycle and DSP reports, view waveform scopes, capture live output and run independent offline exports. See the [engine API](https://niminal.dev/reference/engine-api/) and [client examples](clients/examples/).
- **Offline** — `niminal-live render` renders a piece to audio; `niminal replay` plays a log of timestamped engine events to identical samples.
- **Musical** — Rational time, mini-notation, clips, scenes, and arrangements; one pattern cycle is one bar.
- **Routing** — Independent fx instances, private buses and sample-timed connection edits; client-owned mixer models.
- **Extensible** — Write functions (down to the sample, with state) and instruments in the language, with the same tooling as built-ins.

## Repository layout

| Path | Purpose |
| --- | --- |
| `crates/niminal-cli` | `niminal` command-line tool |
| `crates/niminal-server` | The engine: an audio server that plays events stamped in absolute samples |
| `crates/niminal-live` | The live sequencer: clips, scenes, quantize, keyboards; a client of the engine's API |
| `crates/niminal-*` | Language, DSP, LSP, and related libraries |
| `examples/` | Example `.nml` pieces |
| `editors/vscode/` | Editor extension |
| `docs/` | [niminal.dev](https://niminal.dev) site source |

## Feedback

Issues and experiments are welcome while the project is in alpha. If something breaks, a minimal `.nml` file and what you ran (`render`, `server`, etc.) helps a lot.

## License

MIT, see [LICENSE](LICENSE).
