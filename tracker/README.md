# niminal tracker

A Tauri desktop tracker with CodeMirror 6 instrument/effect editors. It connects directly to niminal, independently of nimlive.

## Run

Install the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/), Node.js and Rust. From this directory:

```sh
npm ci
npm run tauri -- dev
```

For audio, run `niminal server` in another terminal, then click **Connect** (port 7401 by default). A token can be entered for authenticated servers; it is not saved. Click **Play loop** to hear the starter pattern, or **Audition** to preview the selected instrument.

The installed macOS release can be opened with:

```sh
open "$HOME/Applications/niminal tracker.app"
```

No separately installed `nimlive` is needed. The Rust backend embeds the same niminal LSP implementation used by `nimlive lsp`; CodeMirror talks JSON-RPC to it through an ordered Tauri transport. `npm run dev` is a browser-only layout preview, with no LSP or audio connection.

## Compose and edit

- Choose an instrument in each track header. Enter notes using **Z S X D C V G B H N J M**, move with arrows, and clear with Delete/Backspace. Octave is adjustable. Entered notes audition while stopped and connected.
- **Space** plays/stops when the grid is focused. **F6** switches between grid and code. Code editing keeps its normal shortcuts; Space in code does not affect playback.
- **Shift+arrows** or Shift+click selects a rectangular block. **Cmd/Ctrl+A** selects all notes. Use normal copy/cut/paste shortcuts to exchange tab-separated note blocks. Paste clips to the pattern's bounds. Delete clears the selection.
- **Cmd/Ctrl+Z** and **Cmd/Ctrl+Shift+Z** undo/redo grid edits. The toolbar buttons do the same. CodeMirror has its own per-definition undo history.
- Set tempo and rows per beat while stopped. The pattern loops over 32 rows. Edits to notes/assignments take effect beyond the scheduler's existing 200 ms lookahead.
- Each track has volume, mute, and one effect slot in the mixer. Choose an effect, then **Edit FX** to open its code. Effects have separate DSP instances for each track, even when they share a definition.
- Select a sound in the sidebar or add an instrument/effect. Hover for compiler documentation, Ctrl+Space for completion, and Shift+Alt+F to format. Change `saw` to `sawz` to try error diagnostics.
- Click **Apply** to compile code into the engine. Rejected edits leave the last applied source sounding. Apply updates every track using an effect with a short crossfade. **Audition** on an effect plays an assigned track's instrument through it; on an instrument it uses the grid's current track.
- An instrument slot currently needs one instrument with a `freq: hz` parameter. An effect slot needs one `fx name { … }`, using `it` for incoming audio. Helper functions/constants may accompany either. Top-level sound names must be unique across slots.
- Edits autosave locally. **Save/Open project** use native JSON file dialogs. Source, notes, assignments, tempo, volume and mute are saved. Opening a project disconnects the previous session so definitions cannot leak between songs. The project format is version 2; the original version 1 editor sketch is not imported.

## Timing and scope

A dedicated Rust worker schedules against the engine sample clock. Every row timestamp is derived from one origin, avoiding accumulated rounding drift across loops. UI timers only paint the playhead. Stop cancels pending pattern events before silencing this connection. Disconnect and application exit release the connection's engine resources.

The scheduler works 200 ms ahead. A sufficiently long compile or transport stall can exhaust that window; missed rows are skipped rather than played as a burst, and the UI reports skipped rows and engine-reported late events. Tempo changes require stopping playback in this version.

Not implemented yet: multiple patterns/song order, sample editing, parameter columns/automation, effect chains/sends, meters, and a reusable preset library. There is one note column per track and one effect per track. Pattern undo covers note edits, not mixer or source changes.

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

On macOS the app is at `src-tauri/target/release/bundle/macos/niminal tracker.app`. The tracker is its own Cargo workspace so Tauri dependencies stay out of the engine/sequencer builds. Its integration tests use the real WebSocket engine and rendered samples to verify auditioning, loop cancellation, last-good code, effect replacement, volume and mute.
