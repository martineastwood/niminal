# niminal tracker

A Tauri desktop tracker with CodeMirror 6 instrument/effect editors. It connects directly to niminal, independently of nimlive.

## Run

Install the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/), Node.js and Rust. From this directory:

```sh
npm ci
npm run tauri -- dev
```

For audio, run `niminal server` in another terminal, then click **Connect** (port 7401 by default). A token can be entered for authenticated servers; it is not saved. Click **Loop pattern** to hear the current pattern, or **Audition** to preview the selected instrument.

The installed macOS release can be opened with:

```sh
open "$HOME/Applications/niminal tracker.app"
```

No separately installed `nimlive` is needed. The Rust backend embeds the same niminal LSP implementation used by `nimlive lsp`; CodeMirror talks JSON-RPC to it through an ordered Tauri transport. `npm run dev` is a browser-only layout preview, with no LSP or audio connection.

## Compose and edit

The pattern grid is the main workspace. **Track effects** and **Code editor** toggle independent panels; both start minimised. **Focus pattern** hides both panels and closes the sound library. Selecting a sound or pressing F6 opens its code editor. Each panel also has a **Minimise** button. Clicking the same track’s **{}** or **FX** button again closes its corresponding panel.

The left-hand sequence controls song order. Each step occupies one compact row; the **◂ / ▸** button hides or shows pattern names. Hover over the small action buttons for their labels.

- **+ Blank** creates an empty pattern and inserts it after the selected sequence step.
- **Reuse pattern** inserts another reference to the current pattern. Edits affect every entry referencing it; the pattern toolbar shows the reference count.
- **Duplicate as variation** creates an independent copy and inserts it after the selected step.
- Use the pattern selector on a sequence entry to change what it plays. **↑ Move / ↓ Move** reorder entries; **Remove step** removes a reference while retaining its pattern in the pattern pool. At least one sequence entry remains.
- The pattern picker above the grid can open any pattern, including one not currently sequenced. Patterns have a name and **1–256 active rows**. Shortening a pattern preserves hidden notes so lengthening it restores them.
- **Loop pattern** repeats the currently edited pattern. **Play song** starts at the selected sequence entry and stops after the final entry, allowing releases and effect tails to finish. **Stop** cancels queued events and silences the connection.
- **Follow playback** moves the editor to the sounding pattern at each sequence boundary. Selecting a pattern manually turns follow off so you can edit another section while listening. The sounding sequence entry remains highlighted.
- Instruments, effects, volume and mute are song-wide track settings. Patterns contain notes and parameter values. Stop playback before changing sequence structure, pattern lengths, effect-chain structure or tempo; notes, parameter values and parameter values remain live.

- Tracks have compact, colour-coded headers. Click a track name to select it, **double-click its name to rename** (Enter or click away saves; Escape cancels), **{}** to open its instrument code (or create an instrument for an empty slot), **FX** to reveal its bottom device strip, and **M** to mute. **+ Track** adds a song-wide track while stopped, up to 16; scroll horizontally to reach tracks outside the viewport. The row-number gutter stays visible. Choose an instrument in each track header. **− Track** deletes the selected track across all patterns while stopped; **↶** restores the last deleted track, including its notes and mixer settings. At least one track remains. Track deletion/restoration resets note undo histories because column indices change. Enter notes using **Z S X D C V G B H N J M**, move with arrows, and clear with Delete/Backspace. **Step** sets how many rows to advance after note entry (0–256): 4 at four rows per beat enters notes on consecutive beats, 16 on consecutive bars, and 0 stays on the current row. Arrows still move by one row. Octave is adjustable. Entered notes audition while stopped and connected.
- **Space** plays/stops when the grid is focused. **F6** switches between grid and code. Code editing keeps its normal shortcuts; Space in code does not affect playback.
- Click and drag across notes, or use **Shift+arrows** / Shift+click, to select a rectangular block. **Cmd/Ctrl+A** selects all notes. Use normal copy/cut/paste shortcuts to exchange tab-separated note blocks. Paste clips to the pattern's bounds. Delete clears the selection.
- **Cmd/Ctrl+Z** and **Cmd/Ctrl+Shift+Z** undo/redo grid edits. The toolbar buttons do the same. CodeMirror has its own per-definition undo history.
- Set tempo and rows per beat while stopped. Patterns can have different lengths; song playback transitions at their row boundaries. Edits to notes/assignments take effect beyond the scheduler's existing 200 ms lookahead.
- Each track has pre-volume, pan, mute, and up to eight numbered insert effects in the horizontally scrolling bottom strip. Choose from **Built-in effects** or **Project effects** and click **+ Insert effect**. The built-in library includes stereo delay, chorus, low-pass, high-pass, band-pass and notch filters. Inserting a built-in creates a project-owned editable code definition; inserting an existing project effect reuses its definition. **+ Custom effect** creates and inserts an effect and opens its code editor. Use the card arrows to reorder effects and **×** to remove an insert while stopped. **{} Edit** opens its code. Each insert is a separate niminal-native effect instance, including repeated uses of the same definition.
- Select a sound in the sidebar or add an instrument/effect. Hover for compiler documentation, Ctrl+Space for completion, and Shift+Alt+F to format. Change `saw` to `sawz` to try error diagnostics.
- Click **Apply** to compile code into the engine. Rejected edits leave the last applied source sounding. Apply updates every track using an effect with a short crossfade. **Audition** on an effect plays an assigned track's instrument through it; on an instrument it uses the grid's current track.
- An instrument slot currently needs one instrument with a `freq: hz` parameter. An effect slot needs one `fx name { … }`, using `it` for incoming audio. Helper functions/constants may accompany either. Top-level sound names must be unique across slots.
- Edits autosave locally. **Save/Open project** use native JSON file dialogs. Source, notes, assignments, tempo, volume and mute are saved. Opening a project disconnects the previous session so definitions cannot leak between songs. The project format is version 5, with effect chains, parameter values, LFO settings and named parameter lanes alongside notes. Previous file formats are not imported.

## Parameter lanes and modulation

Each track starts with a **Track volume** lane. Use the lane’s header dropdown to choose **Track volume**, **Track pan**, or a named parameter such as **Stereo delay → feedback**. Connect and Apply effects to expose their parameters. The small **+ / −** controls add/remove up to four lanes per track per pattern. A lane with values must be cleared before retargeting or removing it. Each parameter can have one lane per track.

Click a cell and type a readable value, then **Enter** to save and advance by **Step**. **Escape** cancels. Clicking away saves without advancing. Arrows navigate cells; Enter or a number starts keyboard editing. Blank cells hold the previous value, including across pattern boundaries; playback starts from saved manual settings. Delete clears selected values.

- Volume: `-6 dB` (range −60 to +6 dB).
- Pan: `-100%` left, `0%` centre, `100%` right.
- Frequency: `2400 Hz` or `2.4 kHz`.
- Time: `250 ms` or `0.25 sec`; bare numbers mean milliseconds.
- A parameter declared `0..1`: `35%`; bare numbers mean percent.
- Other parameters: their physical value and optional unit, as shown on the slider.

Populated cells have a **⋮** handle: drag vertically to adjust; hold Shift for finer movement. One drag is one undoable edit. Click/drag elsewhere selects rectangles as for notes. Copy/paste uses readable values with units and rejects values outside the destination lane’s range. No hex commands or 8-bit value quantization are used.

Connect and **Apply** an effect to expose its scalar parameters. For example:

```niminal
fx soften(cutoff: hz = 2400hz, resonance: 0..1 = 0.2) {
  out = it.lpf(cutoff: cutoff, res: resonance)
}
```

Move a slider or edit its numeric value for live control. Built-in and custom effects use the same generated panel. **↳**, or right-clicking a slider, writes the exact value into a matching lane on that effect’s track at the current row. It reuses a matching lane, assigns an empty unbound lane or adds a new lane if space permits. Lane targets follow inserts when reordered. Removing/replacing an insert clears its parameter lanes.

Use **∿ LFO** beside a parameter to add/remove modulation. Choose sine, triangle, saw or square, lines per cycle, and normalized minimum/maximum. These are niminal-native oscillator nodes feeding native modulation buses and parameter mappings. They run in the engine, including during stopped auditions; Play schedules a native phase reset at the playback origin. Bus mapping has niminal's normal block delay. A row value targeting an LFO-controlled parameter disconnects that mapping and takes control until Play resets it or the LFO is toggled off/on. Other parameters keep modulating.

Starting/stopping restores the saved manual volume/pan/parameter values rather than leaving the last pattern value latched as the next play's starting point. LFO mappings continue to own their targets. Structural chain changes require stopping, while live parameter and LFO edits use niminal's parameter/routing protocol.

The tracker does not implement an audio engine, buffers, channels, oscillators or filters. See [audio ownership and future send tracks](ARCHITECTURE.md).

## Graphical automation

Click **Automation** in the top bar, or **⌁** beside a named lane header, to open the lower envelope editor. It can be minimised independently; **Focus pattern** hides it along with the other panels. Resize it using its lower-right corner.

Choose a track and one of its named parameter lanes. **Use envelope** switches that lane between its saved row values and the envelope; both are retained in the project. The active envelope is shown in blue in the pattern grid. Clicking those cells opens the envelope editor. Slider **↳** switches the matching lane back to row values.

- **Points**: click to add a point, drag a point to move it, right-click to delete it.
- **Draw**: click and drag to paint a sequence of points.
- **Snap**: row, beat, or 1/16 row. The horizontal axis is pattern time; the vertical axis uses the target parameter’s range.
- **Linear** ramps continuously between points; **Hold** changes only at points. Native niminal parameter glides generate the ramps. Volume/dB controls interpolate in linear gain, which the graph displays as a curve on its dB axis.
- Select a point and use **Row** / **Value** for exact placement and unit-aware values. **Delete point** removes it.
- Undo/redo buttons cover envelope edits; Cmd/Ctrl+Z works with the graph focused. One drag/draw gesture is one undo entry. **Clear envelope** removes the points and returns to row values.

Envelopes belong to a pattern and repeat with it. Reused pattern references share the same envelope; duplicating a variation makes an independent copy. Pattern shortening retains hidden points. Before its first point, a parameter retains its current value; after the last, it holds that value until another event changes it. Envelopes take control from an LFO at their first active segment. Envelopes and row values never simultaneously control one lane.

Editing during playback affects newly scheduled row intervals; queued intervals retain their earlier shape. At slow tempos that can take up to one row beyond the lookahead. No envelope DSP runs in a UI timer or in the tracker audio path.

## Timing and scope

A dedicated Rust worker schedules against the engine sample clock. Every row timestamp is derived from one origin, avoiding accumulated rounding drift across loops. UI timers only paint the playhead. Stop cancels pending pattern events before silencing this connection. Disconnect and application exit release the connection's engine resources.

The scheduler works 200 ms ahead. A sufficiently long compile or transport stall can exhaust that window; missed rows are skipped rather than played as a burst, and the UI reports skipped rows and engine-reported late events. Tempo changes require stopping playback in this version.

Not implemented yet: curved envelope interpolation, per-note modulation UI, sample editing, auxiliary send/return UI, meters, and a reusable preset library. Pattern undo covers note and parameter-cell edits, not mixer or source changes.

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
