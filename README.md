# niminal

**Make music by writing it.** niminal is a live-coding music system with its own audio language. Describe instruments, effects, patterns and arrangements in plain text, hear them instantly, and change them while they play.

> **Alpha.** APIs, syntax, and behavior change without notice. Expect rough edges and the occasional crash. Not ready for production or long-term projects yet.

```
tempo 110bpm

instr pluck(freq: hz, bright: 0..1 = 0.5) {
  level = perc(decay: 400ms)
  out = osc(saw, freq).lpf(cutoff: freq * (2 + bright * 8), res: 0.25) * level
}

clip bass {
  instrument: pluck(bright: 0.2)
  notes:  [c2 ~ c2 eb2 ~ g1 bb1 ~]
}
```

[Documentation](https://niminal.dev) · [Language reference](https://niminal.dev/reference/language/) · [Releases](https://github.com/martineastwood/niminal/releases)

## Why niminal

- **One language, from oscillator to arrangement.** Build an instrument out of oscillators and filters, write the pattern that plays it, and arrange clips into scenes, all in the same file with the same tools. Units like `hz`, `db` and `ms` are part of the language, so a mistake is caught before it reaches your speakers.
- **Change anything while it plays.** Evaluate a block and it lands on the next beat or bar you choose, so a set can evolve without stopping and without glitches.
- **Your own DSP, down to the sample.** Write functions with state that run at audio rate, and use them exactly like the built-ins.
- **Expressive patterns.** Mini-notation, per-note controls, rational time and clips that you can transpose, reverse, mute and re-pattern on the fly.
- **Exactly repeatable.** Render a piece to a WAV file offline, or replay a recorded live session sample for sample and get identical audio every time.
- **Pick the way you like to work.** Code in Stage, or in VS Code playing through Stage. One app, with the audio engine inside it.
- **Open to your own tools.** The audio engine speaks a WebSocket API with a JavaScript client, so your own programs can play through it too (Stage serves it, or run `niminal server`).
- **Fast and parallel.** A Rust engine with voice-parallel DSP, MIDI and OSC control, and MIDI clock in and out.

## How it fits together

**Stage** is the app. It has the audio engine and the live sequencer inside it, so you open it and play. The **VS Code extension** sends the code you evaluate to Stage, and Stage's lanes and scenes show it.

```
  VS Code extension ──▶ Stage ◀── your code (JS, ...)
                        ├ live sequencer: tempo, clips, scenes, patterns
                        └ audio engine: compiles instruments and plays them
```

The engine compiles instruments, effects and routing, and plays timestamped events. Clients decide what to play and when: tempo, clips, scenes and patterns live in the sequencer. For testing and for building your own clients, `niminal server` runs the same engine as a command.

| Piece | What it is | Needs |
| --- | --- | --- |
| [Stage](https://niminal.dev/reference/stage/) | Desktop app for live coding: editor, scenes, lanes that light up as clips play, meters, controls | nothing: the engine is built in |
| [VS Code / Cursor extension](https://niminal.dev/reference/editor/) | Highlighting, diagnostics, hover, completion, evaluate-while-playing | Stage installed (and open, for playback) |
| [`niminal`](https://niminal.dev/reference/server/cli/) | The command line: `play` a file and hear it change as you edit, `render` to WAV, `server` for your own clients, `replay`, and the language server | nothing |

## Install

### macOS download (recommended)

The alpha is a single `Niminal` folder (Apple Silicon, macOS 13+) with Stage, the `niminal` command, the VS Code extension and examples. No Rust, Node or installer needed.

1. Download the latest dmg from [GitHub Releases](https://github.com/martineastwood/niminal/releases) and drag the `Niminal` folder onto Applications, so it ends up at `/Applications/Niminal`.
   Or with Homebrew, which also puts `niminal` on your PATH and skips the macOS warning below:

   ```sh
   brew tap martineastwood/niminal
   brew trust --cask martineastwood/niminal/niminal   # recent Homebrew only
   brew install --cask martineastwood/niminal/niminal
   ```

2. The apps are ad-hoc signed and not notarized. If macOS blocks Stage or `niminal` on first use, approve it under **System Settings > Privacy & Security > Open Anyway**.
3. To use `niminal` from Terminal, put it on your PATH (add the line to `~/.zprofile` to keep it):

   ```sh
   export PATH="/Applications/Niminal/bin:$PATH"
   ```

The bundled `Getting Started.txt` covers the same steps, plus upgrading and uninstalling.

### From source

You need a recent [Rust toolchain](https://rustup.rs/). From a clone of this repo:

```sh
cargo install --path sequencer/live-cli      # niminal: play, render, server, replay and the language server
```

To build Stage from source you also need Node.js and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). See [stage/](stage/).

## Run it

### 1. Hear something (no server needed)

Offline rendering works on its own. Write a piece to a WAV file:

```sh
niminal render examples/saw_lead.nml --out saw_lead.wav
```

### 2. Play live

Open **Niminal Stage**, open a `.nml` file (try `examples/groove.nml`) and press **Cmd+Enter** to evaluate a block. The engine runs inside Stage and plays through your default sound card, so there is nothing to start first.

**VS Code / Cursor**: choose **File > Install VS Code Extension…** in Stage (or accept the offer the first time it opens), or install `niminal-vscode.vsix` yourself (or `editors/vscode`). Then open a `.nml` file and press **Cmd+Enter** (**Ctrl+Enter**) to evaluate a block, **Cmd+.** to hush. The code plays through Stage, which opens if it isn't running, and its clips and scenes appear in Stage's lanes. Diagnostics, hover and completion come from the Stage app and work even when Stage is not open.

### 3. Build on the engine

To drive the engine from your own program, run it on its own:

```sh
niminal server
```

It listens on port 7401 (Stage serves the engine on 7401 too when it is open, with the sequencer on 7400). Ctrl+C stops it. See `niminal server --help` for audio and MIDI device options.

If a client says it cannot connect, check that the server is running and that you restarted it after upgrading, since it keeps the old build until restarted.

### Record and replay

Live sessions are logged sample-accurately, so you can turn any performance into audio later:

```sh
niminal replay ~/.niminal/sessions/<log> --out set.wav
```

For MIDI / OSC control, start the live client with `--midi` and/or `--osc-port` (see `examples/midi_osc.nml`). Use `--help` for all flags.

## What you get

- **Live** — Evaluate while playing, quantized to musical time, from Stage or VS Code.
- **Offline** — `niminal render` renders a piece to audio; `niminal replay` reproduces a session to identical samples.
- **Musical** — Rational time, mini-notation, clips, scenes, and arrangements; one pattern cycle is one bar.
- **Routing** — Independent fx instances, private buses and sample-timed connection edits.
- **Extensible** — Write functions and instruments in the language, with the same tooling as built-ins.
- **Open API** — Browser and Node clients can send events, define instruments, effects and routing, and subscribe to the clock, meters and voices. See the [engine API](https://niminal.dev/reference/engine-api/).

## Repository layout

| Path | Purpose |
| --- | --- |
| `crates/niminal-cli` | `niminal` command-line tool |
| `crates/niminal-server` | The engine: an audio server that plays events stamped in absolute samples |
| `sequencer/live` | The live sequencer: clips, scenes, quantize, keyboards; plays through the engine in-process |
| `stage/` | Niminal Stage, the desktop app: the engine and the sequencer, with an editor and lanes |
| `crates/niminal-*` | Language, DSP, LSP, and related libraries |
| `examples/` | Example `.nml` pieces |
| `editors/vscode/` | Editor extension |
| `docs/` | [niminal.dev](https://niminal.dev) site source |

## Feedback

Issues and experiments are welcome while the project is in alpha. If something breaks, a minimal `.nml` file and what you ran (`render`, `server`, etc.) helps a lot.

## License

MIT, see [LICENSE](LICENSE).
