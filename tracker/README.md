# niminal tracker

A Tauri desktop tracker with CodeMirror 6 instrument/effect editors. It connects directly to niminal, independently of nimlive.

## Run

Install the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/), Node.js and Rust. From this directory:

```sh
npm ci
npm run tauri -- dev
```

For audio, run `niminal server` in another terminal, then click **Connect** in the footer (port 7401 by default). A token can be entered for authenticated servers; it is not saved. Click **▶ Pattern** to hear the current pattern. Entering a note in the grid previews it through the track's instrument and FX while stopped.

The installed macOS release can be opened with:

```sh
open "$HOME/Applications/niminal tracker.app"
```

No separately installed `nimlive` is needed. The Rust backend embeds the same niminal LSP implementation used by `nimlive lsp`; CodeMirror talks JSON-RPC to it through an ordered Tauri transport. `npm run dev` is a browser-only layout preview, with no LSP or audio connection.

## Compose and edit

The pattern grid is the main workspace. A single toolbar holds transport, tempo (BPM, LPB), octave, entry step, pattern and track controls; engine connection and status live in the footer. The bottom dock has **Code** and **Automation** tabs and starts closed. Click **{}** in a track header to edit its instrument, **FX** to edit its processor (presets and generated controls sit beside the code), or **⌁** in a lane header to open that lane’s envelope. **×** closes the dock, **⤢** expands it over the grid, and dragging its top edge resizes it. F6 switches between the grid and the editor.

The left-hand sequence controls song order. Each step occupies one compact row; the **◂ / ▸** button hides or shows pattern names. Hover over the small action buttons for their labels.

- **+ Blank** creates an empty pattern and inserts it after the selected sequence step.
- **Reuse pattern** inserts another reference to the current pattern. Edits affect every entry referencing it; the pattern toolbar shows the reference count.
- **Duplicate as variation** creates an independent copy and inserts it after the selected step.
- Use the pattern selector on a sequence entry to change what it plays. **↑ Move / ↓ Move** reorder entries; **Remove step** removes a reference while retaining its pattern in the pattern pool. At least one sequence entry remains.
- The pattern picker above the grid can open any pattern, including one not currently sequenced. Patterns have a name and **1–256 active rows**. Shortening a pattern preserves hidden notes so lengthening it restores them.
- **▶ Pattern** repeats the currently edited pattern. **▶ Song** starts at the selected sequence entry and stops after the final entry, allowing releases and effect tails to finish. **■** cancels queued events and silences the connection.
- **Follow playback** moves the editor to the sounding pattern at each sequence boundary. Selecting a pattern manually turns follow off so you can edit another section while listening. The sounding sequence entry remains highlighted.
- Instruments, effects, volume and mute are song-wide track settings. Patterns contain notes and parameter values. Stop playback before changing sequence structure, pattern lengths, track assignments or tempo; notes and parameter values remain live.

- Tracks have compact, colour-coded headers. Click a track name to select it, **double-click its name to rename** (Enter or click away saves; Escape cancels), **{}** to open its instrument code (or create an instrument for an empty slot), **FX** to edit its whole track processor, and **M** to mute. **+ Track** adds a song-wide track while stopped, up to 16; scroll horizontally to reach tracks outside the viewport. The row-number gutter stays visible. Choose an instrument in each track header. **− Track** deletes the selected track across all patterns while stopped; **↶** restores the last deleted track, including its notes and mixer settings. At least one track remains. Track deletion/restoration resets note undo histories because column indices change. Enter notes using **Z S X D C V G B H N J M**, move with arrows, and clear with Delete/Backspace. **Step** sets how many rows to advance after note entry (0–256): 4 at four rows per beat enters notes on consecutive beats, 16 on consecutive bars, and 0 stays on the current row. Arrows still move by one row. Octave is adjustable. Entered notes audition while stopped and connected.
- **Space** plays/stops when the grid is focused. **F6** switches between grid and code. Code editing keeps its normal shortcuts; Space in code does not affect playback.
- **Record mode** (**● Rec** in the toolbar, or **Escape** with the grid focused) controls editing. With it off, the keyboard only previews notes on the selected track's instrument, and typing, Delete, cut, paste and value editing do nothing. With it on, a red frame surrounds the grid and notes and values are entered at the cursor. Corner brackets and a tinted header always mark the selected track.
- **Notes** are held until the next note or an **OFF** in the same column: press **1** (or the backtick key) in a note column to enter OFF. There is no fixed note length, so leave a gap with OFF for staccato. A looped pattern holds a lone note for the whole pattern, and a song holds the last note until it ends.
- **Note columns and sub-columns.** The **⋯** button in a track header adds up to eight note columns (for chords, shared by every pattern) and shows or hides the **Ins**, **Vol**, **Pan** and **Dly** sub-columns beside each note. They take two hex digits: **Ins** is an instrument number from the sidebar list, which plays that note on another instrument; **Vol** runs 00–80 (80 is full level); **Pan** runs 00 left, 40 centre, 80 right; **Dly** starts the note late by that many 256ths of a row. Type the second digit to move down; the first digit alone is a valid value. Hidden sub-columns still play.
- **Instrument parameters.** Editing an instrument shows its parameters as sliders beside the code (after **Apply**), for any parameter except `freq`, `gain` and `pan`. Notes start with those values. **↳** records a value into a lane on a track that plays the instrument; from that row on, notes start with the lane's value (or envelope value) instead. The lane acts when a note starts, so it does not sweep a note that is already sounding.
- Click and drag across notes, or use **Shift+arrows** / Shift+click, to select a rectangular block. **Cmd/Ctrl+A** selects all notes. Use normal copy/cut/paste shortcuts to exchange tab-separated note blocks. Paste clips to the pattern's bounds. Delete clears the selection.
- **Cmd/Ctrl+Z** and **Cmd/Ctrl+Shift+Z** undo/redo grid edits. The toolbar buttons do the same. CodeMirror has its own per-definition undo history.
- Set tempo and rows per beat while stopped. Patterns can have different lengths; song playback transitions at their row boundaries. Edits to notes/assignments take effect beyond the scheduler's existing 200 ms lookahead.
- **Instruments** are listed in the sidebar: **+** adds a synth, **⧉** duplicates the selected one (with fresh names inside its code), **−** deletes it unless a track plays it, and the dock's name field renames it. **♪** imports WAV files: each is copied into the project's folder and becomes a sampler instrument written as ordinary code (`sample("samples/kick.wav").root(c4)`), so root note, looping and tuning are edited in the editor. The engine resolves sample paths against the project folder, so it needs to run on the same machine.
- Each track has volume, pan, mute, and an **effect chain** of up to eight devices, run in order. Click **FX** in a track header to open the chain: the **+ Add effect…** list appends a built-in preset (stereo delay, chorus, low/high/band-pass, notch, tremolo) or a blank device, each with its own copy of the code. A device is a card with bypass (⏻), move (↑ ↓) and remove (×) buttons; click one to edit its code and see its sliders, LFOs and ↳ buttons. Adding a preset applies it at once (while stopped or playing); press **Apply** after editing the code yourself. A bypassed device is removed from the engine graph, and lanes that target it pause. The last working code keeps sounding if compilation fails.
- Select a sound in the sidebar or add an instrument/effect. Hover for compiler documentation, Ctrl+Space for completion, and Shift+Alt+F to format. Change `saw` to `sawz` to try error diagnostics.
- Click **Apply** to compile code into the engine. Rejected edits leave the last applied source sounding. Apply updates every track using an effect with a short crossfade.
- An instrument slot currently needs one instrument with a `freq: hz` parameter. An effect slot needs one `fx name { … }`, using `in` for incoming audio. Helper functions/constants may accompany either. Top-level sound names must be unique across slots.
- Edits autosave locally. **Save** writes a project folder (`name.nimtracker/` with `project.json` and a `samples/` folder) and **Open** takes its `project.json`. Source, notes, assignments, tempo, volume, mute, effect chains, LFO settings and parameter lanes are saved. Opening a project disconnects the previous session so definitions cannot leak between songs. The project format is version 8; earlier formats are not imported.

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
  out = in.lpf(cutoff: cutoff, res: resonance)
}
```

Move a slider or edit its numeric value for live control. Built-in and custom effects use the same generated panel. **↳**, or right-clicking a slider, writes the exact value into a matching lane on that effect’s track at the current row. It reuses a matching lane, assigns an empty unbound lane or adds a new lane if space permits. Automation addresses parameter names within the track. Changing code or loading a preset preserves manual values and lanes. Renamed/removed parameters are labelled missing and skipped until repaired or restored.

Use **∿ LFO** beside a parameter to add/remove modulation. Choose sine, triangle, saw or square, lines per cycle, and normalized minimum/maximum. These are niminal-native oscillator nodes feeding native modulation buses and parameter mappings. They run in the engine, including during stopped auditions; Play schedules a native phase reset at the playback origin. Bus mapping has niminal's normal block delay. A row value targeting an LFO-controlled parameter disconnects that mapping and takes control until Play resets it or the LFO is toggled off/on. Other parameters keep modulating.

Starting/stopping restores the saved manual volume/pan/parameter values rather than leaving the last pattern value latched as the next play's starting point. LFO mappings continue to own their targets. Source edits use Apply; live parameter and LFO edits use niminal's parameter/routing protocol.

The tracker does not implement an audio engine, buffers, channels, oscillators or filters. See [audio ownership and future send tracks](ARCHITECTURE.md).

## Graphical automation

Click **⌁** beside a lane header to open the envelope editor in the dock’s **Automation** tab.

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

Not implemented yet: curved envelope interpolation, per-note modulation of a sounding note, slices and kits as notes, waveform preview, auxiliary send/return UI, meters, per-note effect commands (parameter lanes cover most of them), and a shared preset library. Pattern undo covers note, sub-column and lane cell edits, not mixer, instrument or chain changes.

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
