# niminal tracker

First desktop prototype: a tracker pattern editor beside a CodeMirror 6 instrument/effect editor. This is an independent niminal client, not a frontend for nimlive.

## Run

Install the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/), Node.js and Rust. From this directory:

```sh
npm ci
npm run tauri -- dev
```

No audio server or separately installed `nimlive` is needed for this editor prototype. The Rust backend embeds the **same niminal LSP implementation** used by `nimlive lsp`; CodeMirror talks JSON-RPC to it through an ordered Tauri transport. Compilation runs off the window thread.

`npm run dev` is a browser-only layout preview. It deliberately reports that LSP is unavailable; use the desktop app to test language features.

## Try it

- Click an instrument or effect to edit it. Add either kind with the sidebar buttons.
- Hover over `osc`, `lpf`, or `tone` for compiler documentation. Ctrl+Space opens completion; Shift+Alt+F formats source. Signature help appears in calls.
- Change `saw` to `sawz` to see an error underline, then hover over it for the message. Undo to clear it.
- Click the grid and enter notes with `Z S X D C V G B H N J M`. Use arrows to move and Delete/Backspace to clear. Octave is adjustable.
- F6 switches between the grid and code editor. Tab indents inside the code editor. Expand editor gives code the whole workspace.
- Edits autosave locally. Save/Open project use native file dialogs in the desktop app and JSON files for portable projects. Each sound's source, selection and undo history survive switching slots during the session.

**Not implemented yet:** engine connection, auditioning, playback/scheduling, instrument assignments to cells, song arrangement, tracker undo/selection, and routing/mixer. The initial grid is an editing sketch, with one note column per track. Instrument/effect definitions are project-local; a reusable preset library comes later.

## Check and build

From the repository root:

```sh
scripts/check.sh --test
npm --prefix tracker test
npm --prefix tracker run build
cargo clippy --manifest-path tracker/src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path tracker/src-tauri/Cargo.toml
cargo install --path crates/niminal-cli
```

Build the desktop release from this directory:

```sh
npm run tauri -- build
```

On macOS the app is at `src-tauri/target/release/bundle/macos/niminal tracker.app`. The tracker is its own Cargo workspace so Tauri dependencies stay out of the engine and sequencer builds.
