# niminal

**Alpha.** APIs, syntax, and behavior change without notice. Expect rough edges, missing pieces, and the occasional crash. Not ready for production or long-term projects yet.

niminal is a live-coding audio language with its own DSP engine. You describe instruments, patterns, and scenes in `.nml` files, hear edits land on the beat while a session runs, and render the same file to WAV offline.

[Documentation](https://niminal.dev) · [Language reference](https://niminal.dev/reference/language/) · [GitHub](https://github.com/martineastwood/niminal)

## Install

You need a recent [Rust toolchain](https://rustup.rs/).

From a clone of this repo:

```sh
cargo install --path crates/niminal-cli
```

Or install directly from GitHub:

```sh
cargo install --git https://github.com/martineastwood/niminal niminal-cli
```

## Hear something quickly

The repo includes examples under `examples/`. Render one to a WAV file:

```sh
niminal render examples/saw_lead.nml --out saw_lead.wav
```

Open `saw_lead.nml` to see the language: tempo, instruments with units (`hz`, `db`, `ms`), envelopes, and scored notes.

## Live coding

Start the engine with an initial file, then send more code from the REPL or CLI:

```sh
niminal daemon examples/groove.nml
niminal repl
# in another terminal:
niminal send 'launch chorus' --quantize "next 4 bars"
```

Sessions are logged sample-accurately, so you can replay them:

```sh
niminal render --replay ~/.niminal/sessions/<log> --out set.wav
```

Use `niminal <command> --help` for flags (`render`, `daemon`, `repl`, `send`, `lsp`).

## Editor

The [VS Code / Cursor extension](editors/vscode/) adds syntax highlighting, diagnostics, and evaluate-while-playing against a running daemon. Install it from `editors/vscode` (see that folder’s README), run `niminal daemon`, then evaluate with **Cmd+Enter** (macOS) or **Ctrl+Enter**.

The language server (`niminal lsp`) works without a daemon for hover, completion, and errors.

## What you get

- **Live** — WebSocket daemon, REPL, `send`, and editor integration; quantize changes to musical time.
- **Offline** — Same `.nml` files render to audio; session logs replay to identical samples.
- **Musical** — Rational time, mini-notation, clips, scenes, and arrangements; one pattern cycle is one bar.
- **Extensible** — Define opcodes and instruments in the language with the same tooling as built-ins.

## Repository layout

| Path | Purpose |
| --- | --- |
| `crates/niminal-cli` | `niminal` command-line tool |
| `crates/niminal-*` | Language, engine, daemon, LSP, and related libraries |
| `examples/` | Example `.nml` pieces |
| `editors/vscode/` | Editor extension |
| `docs/` | [niminal.dev](https://niminal.dev) site source |

## Feedback

Issues and experiments are welcome while the project is in alpha. If something breaks, a minimal `.nml` file and what you ran (`render`, `daemon`, etc.) helps a lot.
