# niminal

**Alpha.** APIs, syntax, and behavior change without notice. Expect rough edges, missing pieces, and the occasional crash. Not ready for production or long-term projects yet.

niminal is a sound server with its own DSP language, in the way scsynth is a sound server. It compiles instruments, effects and routing, and plays timestamped events sent to it by any client. It knows nothing of clips, scenes, patterns or tempo.

`nimlive` is the first client: a live-coding sequencer (the text-based Ableton) that adds clips, scenes, mini-notation patterns, tempo and quantized changes on top, and a VS Code extension to drive it. A tracker or a richer UI would be other clients of the same API.

```
crates/      the sound server (scsynth): engine, DSP language, daemon, `niminal`
sequencer/   a client (sclang and its patterns): the composition language, patterns, `nimlive`
editors/     the VS Code extension, a client of `nimlive`
clients/     JS and Python SDKs for the server's API
```

The server's crates never depend on `sequencer/`; it is a separate Cargo workspace that depends on them.

[Documentation](https://niminal.dev) · [Language reference](https://niminal.dev/reference/language/) · [GitHub](https://github.com/martineastwood/niminal)

## Install

You need a recent [Rust toolchain](https://rustup.rs/).

From a clone of this repo, install the sound server and the live sequencer:

```sh
cargo install --path crates/niminal-cli      # niminal
cargo install --path sequencer/nimlive       # nimlive
```

## Hear something quickly

The repo includes examples under `examples/`. Render one to a WAV file:

```sh
nimlive render examples/saw_lead.nml --out saw_lead.wav
```

Open `saw_lead.nml` to see the language: tempo, instruments with units (`hz`, `db`, `ms`), envelopes, and scored notes.

## Live coding

Start the sound server, then the live sequencer client:

```sh
niminal daemon
# in another terminal:
nimlive live examples/groove.nml
nimlive repl
# in another terminal:
nimlive send 'chorus.start' --quantize "next 4bars"
```

Sessions are logged sample-accurately, so you can replay them:

```sh
niminal replay ~/.niminal/sessions/<log> --out set.wav
```

For MIDI / OSC controls, start the live client with `--midi` and/or `--osc-port` (see `examples/midi_osc.nml`). Use `--help` for flags (`niminal`: `daemon`, `replay`, `docs`; `nimlive`: `render`, `live`, `repl`, `send`, `lsp`).

## Editor

The [VS Code / Cursor extension](editors/vscode/) adds syntax highlighting, diagnostics, and evaluate-while-playing against a running daemon. Install it from `editors/vscode` (see that folder’s README), run `niminal daemon`, the extension starts its own private live sequencer, then evaluate with **Cmd+Enter** (macOS) or **Ctrl+Enter**.

The language server (`nimlive lsp`) works without a daemon for hover, completion, and errors.

## What you get

- **Live** — WebSocket daemon, REPL, `send`, and editor integration; quantize changes to musical time. The daemon is a sound server (the engine) with the live sequencer as one client of its event API, so other programs can play through it too. Clients own tempo, meter and all musical scheduling.
- **Frontend API** — Browser, Node and Python clients can own private DSP definitions, route and modulate processors, stream WAV files, inspect voice lifecycle and DSP reports, view waveform scopes, capture live output and run independent offline exports. See the [engine API](docs/src/content/docs/reference/engine-api.md) and [client examples](clients/examples/).
- **Offline** — `nimlive render` renders a piece to audio; `niminal replay` plays a log of timestamped engine events (what scsynth calls non-real-time mode) to identical samples.
- **Musical** — Rational time, mini-notation, clips, scenes, and arrangements; one pattern cycle is one bar.
- **Routing** — Independent processor instances, private buses and sample-timed connection edits; client-owned mixer models.
- **Extensible** — Write functions (down to the sample, with state) and instruments in the language, with the same tooling as built-ins.

## Repository layout

| Path | Purpose |
| --- | --- |
| `crates/niminal-cli` | `niminal` command-line tool |
| `crates/niminal-daemon` | The engine: a sound server that plays events stamped in absolute samples |
| `crates/niminal-live` | The live sequencer: clips, scenes, quantize, keyboards; a client of the engine's API |
| `crates/niminal-*` | Language, DSP, LSP, and related libraries |
| `examples/` | Example `.nml` pieces |
| `editors/vscode/` | Editor extension |
| `docs/` | [niminal.dev](https://niminal.dev) site source |

## Feedback

Issues and experiments are welcome while the project is in alpha. If something breaks, a minimal `.nml` file and what you ran (`render`, `daemon`, etc.) helps a lot.

## License

MIT, see [LICENSE](LICENSE).
