# niminal tracker

A Tauri desktop tracker with CodeMirror 6 instrument/effect editors. It connects directly to niminal, independently of niminal-live.

**The manual lives in the docs site: <https://niminal.dev/reference/tracker/>.** Keep it there — this file is build and architecture notes only.

## Run

Install the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/), Node.js and Rust. From this directory:

```sh
npm ci
npm run tauri -- dev
```

Like Stage, it connects on startup, reconnects automatically if the engine goes away, and connects before applying a new instrument. It never starts an engine: run `niminal server` yourself, and the tracker connects to it on port 7401 once it is listening. An explicit Disconnect stays offline until you connect or create an instrument. `npm run dev` is a browser-only layout preview, with no LSP or audio connection.

No separately installed `niminal-live` is needed: the Rust backend embeds the same niminal LSP implementation used by `niminal-live lsp`, and CodeMirror talks JSON-RPC to it through an ordered Tauri transport. [ARCHITECTURE.md](ARCHITECTURE.md) describes how the tracker maps its tracks, strips and automation onto the engine.

On macOS the installed release opens with:

```sh
open "$HOME/Applications/niminal tracker.app"
```

## Check and build

From the repository root:

```sh
scripts/check.sh --test
npm --prefix tracker test
npm --prefix tracker run build
cargo clippy --manifest-path tracker/src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path tracker/src-tauri/Cargo.toml
cargo install --path crates/niminal-cli
npm --prefix tracker run tauri -- build
```

On macOS the app is at `src-tauri/target/release/bundle/macos/niminal tracker.app`. The tracker is its own Cargo workspace so Tauri dependencies stay out of the engine/sequencer builds. Its integration tests use the real WebSocket engine and rendered samples to verify auditioning, loop cancellation, sequence boundaries/completion, last-good code, effect replacement, volume and mute.
