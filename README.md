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
- **Pick the way you like to work.** Code in Stage or VS Code, or compose in a keyboard-driven tracker grid. They all play through the same audio engine.
- **Open to your own tools.** The audio engine is a WebSocket server with JavaScript and Python clients, so your own programs can play through it too.
- **Fast and parallel.** A Rust engine with voice-parallel DSP, MIDI and OSC control, and MIDI clock in and out.

## How it fits together

niminal has an **audio server** and **clients** that talk to it.

```
  Stage / Tracker / VS Code / your code
                 │   WebSocket (port 7401)
                 ▼
          niminal server      ← compiles instruments, plays the audio
```

The server (`niminal server`) does the sound: it compiles instruments, effects and routing, and plays timestamped events. Clients decide what to play and when: tempo, clips, scenes and patterns live in the client. This is why **the server must be running first**: every client connects to it, and they reconnect automatically if you restart it.

| Piece | What it is | Needs |
| --- | --- | --- |
| `niminal` | The audio server and command-line tool | nothing |
| [Stage](stage/) | Desktop app for live coding: editor, scenes, lanes that light up as clips play, meters, controls | the server |
| [Tracker](tracker/) | Desktop app with a keyboard-driven pattern grid, instrument/effect editor, mixer and project files | the server |
| [VS Code / Cursor extension](editors/vscode/) | Highlighting, diagnostics, hover, completion, evaluate-while-playing | the server for playback only |
| `niminal-live` | The live sequencer and offline renderer | the server for live use only |

## Install

### macOS download (recommended)

The alpha is a single `Niminal` folder (Apple Silicon, macOS 13+) with Tracker, Stage, both command-line tools, the VS Code extension and examples. No Rust, Node or installer needed.

1. Download the latest zip from [GitHub Releases](https://github.com/martineastwood/niminal/releases) and copy the `Niminal` folder to `/Applications/Niminal`.
2. The apps are ad-hoc signed and not notarized. If macOS blocks one on first use, approve it under **System Settings > Privacy & Security > Open Anyway**.
3. Put the tools on your PATH (add the line to `~/.zprofile` to keep it):

   ```sh
   export PATH="/Applications/Niminal/bin:$PATH"
   ```

The bundled `Getting Started.txt` covers the same steps, plus upgrading and uninstalling.

### From source

You need a recent [Rust toolchain](https://rustup.rs/). From a clone of this repo:

```sh
cargo install --path crates/niminal-cli      # niminal (the audio server)
cargo install --path sequencer/live-cli      # niminal-live (sequencer and renderer)
```

To run Stage or the Tracker from source you also need Node.js and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). See [stage/](stage/) and [tracker/README.md](tracker/README.md).

## Run it

### 1. Hear something (no server needed)

Offline rendering works on its own. Write a piece to a WAV file:

```sh
niminal-live render examples/saw_lead.nml --out saw_lead.wav
```

### 2. Start the audio server

For anything live, start the server first, in its own terminal, and leave it running:

```sh
niminal server
```

It listens on port 7401. Ctrl+C stops it. See `niminal server --help` for audio and MIDI device options.

### 3. Connect a client

With the server running, pick one:

- **Stage**: open Niminal Stage, open a `.nml` file (try `examples/groove.nml`) and it connects. Stage has the sequencer built in, so it needs only the server.
- **Tracker**: open Niminal Tracker, create a project, choose an instrument and enter notes in the grid. It connects on startup and never starts a server itself.
- **VS Code / Cursor**: install the extension from `niminal-vscode.vsix` (or `editors/vscode`), open a `.nml` file and press **Cmd+Enter** (**Ctrl+Enter**) to evaluate a block, **Cmd+.** to hush. Diagnostics, hover and completion work even without the server.
- **Terminal**:

  ```sh
  niminal-live live examples/groove.nml
  # in another terminal:
  niminal-live send 'chorus.start' --quantize "next 4bars"
  ```

  `niminal-live repl` gives an interactive prompt.

If a client says it cannot connect, check that `niminal server` is running and that you restarted it after upgrading, since it keeps the old build until restarted.

### Record and replay

Live sessions are logged sample-accurately, so you can turn any performance into audio later:

```sh
niminal replay ~/.niminal/sessions/<log> --out set.wav
```

For MIDI / OSC control, start the live client with `--midi` and/or `--osc-port` (see `examples/midi_osc.nml`). Use `--help` for all flags.

## What you get

- **Live** — Evaluate while playing, quantized to musical time, from Stage, VS Code, the REPL or `send`.
- **Offline** — `niminal-live render` renders a piece to audio; `niminal replay` reproduces a session to identical samples.
- **Musical** — Rational time, mini-notation, clips, scenes, and arrangements; one pattern cycle is one bar.
- **Routing** — Independent fx instances, private buses and sample-timed connection edits.
- **Extensible** — Write functions and instruments in the language, with the same tooling as built-ins.
- **Open API** — Browser, Node and Python clients can define effects, route and modulate them, stream WAV files, capture output and run offline exports. See the [engine API](https://niminal.dev/reference/engine-api/) and [client examples](clients/examples/).

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
