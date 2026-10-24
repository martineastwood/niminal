# niminal tracker

A Tauri desktop tracker with CodeMirror 6 instrument/effect editors. It connects directly to niminal, independently of nimlive.

## Run

Install the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/), Node.js and Rust. From this directory:

```sh
npm ci
npm run tauri -- dev
```

The tracker starts the engine itself: when it opens it connects to `niminal server` on port 7401 (**Connect** in the footer reconnects), and if nothing is listening it runs the `niminal` program it finds on the search path or in `~/.cargo/bin` (`cargo install --path crates/niminal-cli`), and stops it again when it closes. An engine you started yourself is used as it is and left running. Click **▶ Pattern** to hear the current pattern. Entering a note in the grid previews it on the current instrument, through the track's FX, while stopped.

The **menu bar** has File (New, Open, Save, Import Samples), Edit (Undo, Redo, Cut, Copy, Paste), Song, Sound and View. Items do what the matching button or key does; **Cmd/Ctrl+B** shows or hides the sound browser.

The installed macOS release can be opened with:

```sh
open "$HOME/Applications/niminal tracker.app"
```

No separately installed `nimlive` is needed. The Rust backend embeds the same niminal LSP implementation used by `nimlive lsp`; CodeMirror talks JSON-RPC to it through an ordered Tauri transport. `npm run dev` is a browser-only layout preview, with no LSP or audio connection.

## Compose and edit

The pattern grid is the main workspace. A single toolbar holds transport, tempo (BPM, LPB), octave, entry step, pattern and track controls; engine connection and status live in the footer. The bottom dock has **Code** and **Automation** tabs and starts closed. The code pane is open under the grid and shows the current instrument (the one chosen in the sound browser); **Instrument** / **FX** at the top of the pane switch it to the selected track's effect chain (presets and generated controls sit beside the code). **⌁** in a lane header opens that lane’s envelope. **Cmd/Ctrl+Enter** in the editor applies the code and plays a note through it (**Apply ⌘↩** does too). **×** closes the dock, **⤢** expands it over the grid, and dragging its top edge resizes it. F6 switches between the grid and the editor.

The left-hand sequence controls song order. Each step occupies one compact row; the **◂ / ▸** button hides or shows pattern names. Hover over the small action buttons for their labels.

- **+ Blank** creates an empty pattern and inserts it after the selected sequence step.
- **Reuse pattern** inserts another reference to the current pattern. Edits affect every entry referencing it; the pattern toolbar shows the reference count.
- **Duplicate as variation** creates an independent copy and inserts it after the selected step.
- Use the pattern selector on a sequence entry to change what it plays. **↑ Move / ↓ Move** reorder entries; **Remove step** removes a reference while retaining its pattern in the pattern pool. At least one sequence entry remains.
- The pattern picker above the grid can open any pattern, including one not currently sequenced. Patterns have a name and **1–256 active rows**. Shortening a pattern preserves hidden notes so lengthening it restores them.
- **⟲ Loop** repeats part of the sequence during **▶ Song**. Press it to loop the selected step, shift-click another step's number to loop everything between them, and press it again to clear the loop. The looped steps are outlined. Song playback runs from where it starts up to the end of the loop, repeats the loop until you stop it, and ignores a loop that lies before its start. The loop can be set, moved or cleared while playing: playback carries on to the end of the pattern now sounding and loops from there. Steps added inside the loop join it, and the loop follows the sequence as steps are inserted or removed.
- **§ Section** starts a named section at the selected step, running up to the next section's start; press it on a step that starts one to remove it. A section header sits above its steps: **▾ / ▸** collapses or expands the steps (the count shows how many are hidden, and a collapsed section lights up while one of its steps plays), the name is editable, and **⟲** loops exactly that section (press it again to clear). Sections only organise the list and don't change playback. Steps added after a section's first step join it, steps removed move later sections up, and a section left with no steps disappears. Sections are saved with the project.
- **Matrix** (toolbar, or the dock's Matrix tab) is the pattern matrix: a row for each sequence step and a column for each track, so each step can differ per track without copying a whole pattern. A cell normally shows the step's own pattern number. **Click** a cell for a menu to mute that track in that step (**✕**) or have it play another pattern's notes and lanes (**→03**), or choose *Own pattern* to undo; **Shift-click** mutes or unmutes at once. A step label selects the step, and sections appear as separators. A redirected track plays the other pattern's rows from its first row, and falls silent after them if that pattern is shorter than the step. A muted track ends a note still sounding from the step before. The pattern editor marks a track that is muted or redirected in the selected step (the header is dimmed or underlined) because editing its cells there isn't heard. A step's matrix cells move with it when steps are inserted, moved or removed, and with a track when it is deleted and restored. Matrix changes can be made while the song plays. Pattern-only playback (**▶ Pattern**) ignores the matrix.
- **▶ Pattern** repeats the currently edited pattern. **▶ Song** starts at the selected sequence entry and stops after the final entry, allowing releases and effect tails to finish. **■** cancels queued events and silences the connection.
- **Follow playback** moves the editor to the sounding pattern at each sequence boundary. Selecting a pattern manually turns follow off so you can edit another section while listening. The sounding sequence entry remains highlighted.
- Effects, volume and mute are song-wide track settings, and instruments belong to the project, not to tracks. Patterns contain notes and parameter values. Stop playback before changing sequence structure (the loop excepted), pattern lengths, effect chains or tempo; notes and parameter values remain live.

- Tracks have compact, colour-coded headers. Click a track name to select it, **double-click its name to rename** (Enter or click away saves; Escape cancels), **M** to mute, **S** to solo (a dot after the name means the track has effects; the pane's **FX** button opens its chain), and **⋯** for its note columns, sub-columns and parameter lanes. **+ Track** adds a song-wide track while stopped, up to 16; scroll horizontally to reach tracks outside the viewport. The row-number gutter stays visible. **− Track** deletes the selected track across all patterns while stopped; **↶** restores the last deleted track, including its notes and mixer settings. At least one track remains. Track deletion/restoration resets note undo histories because column indices change. Enter notes on the keyboard as a piano: the **Z** row (**Z S X D C V G B H N J M**, continuing **, L . ; /**) plays from the selected octave and the **Q** row (**Q 2 W 3 E R 5 T 6 Y 7 U**, continuing **I 9 O 0 P**) from the octave above, move with arrows, and clear with Delete (Backspace deletes the row instead, as in Renoise). **Step** sets how many rows to advance after note entry (0–256): 4 at four rows per beat enters notes on consecutive beats, 16 on consecutive bars, and 0 stays on the current row. Arrows still move by one row. Octave is adjustable. Entered notes audition while stopped and connected.
- **Space** plays/stops when the grid is focused. **F6** switches between grid and code. Code editing keeps its normal shortcuts; Space in code does not affect playback.
- **Record mode** (**● Rec** in the toolbar, or **Escape** with the grid focused) controls editing. With it off, the keyboard only previews notes on the current instrument, and typing, Delete, cut, paste and value editing do nothing. With it on, a red frame surrounds the grid and notes and values are entered at the cursor. Corner brackets and a tinted header always mark the selected track.
- **Notes** are held until the next note, an **OFF** or a **CUT** in the same column. Press **A** in a note column to enter an OFF, which lets the note release (an instrument with a release stage fades out), or the **minus** key to enter a CUT, which stops it at once with no release. Neither takes an instrument or makes a sound as you enter it, and both advance by the Step like a note. There is no fixed note length, so leave a gap with OFF for staccato. A looped pattern holds a lone note for the whole pattern, and a song holds the last note until it ends.
- **Note columns and sub-columns.** The **⋯** button in a track header adds up to eight note columns (for chords, shared by every pattern) and shows or hides the **Vol**, **Pan** and **Dly** sub-columns beside each note. The **Ins** column is always there: it holds the number of the instrument that plays the note. They take plain numbers, edited like a parameter lane (click a cell or just type a digit, then Enter to save and move down): **Ins** is an instrument's number (shown before its name in the browser's **Project** folder), which plays that note and the notes below it in the column until another is set; **Vol** is 0–100; **Pan** is −50 (left) to +50 (right), or `C` for centre; **Dly** starts the note that many percent of a row late, 0–99. Hidden sub-columns still play.
- **Instrument parameters.** Editing an instrument shows its parameters as sliders beside the code (after **Apply**), for any parameter except `freq`, `gain` and `pan`. Notes start with those values. **↳** records a value into a lane on the selected track for that instrument (the lane picker lists the parameters of the instruments that track plays, and any a lane already points at; a lane for an instrument no note on the track uses is dimmed, because it has no effect); from that row on, notes of that instrument start with the lane's value (or envelope value) instead. The lane acts when a note starts, so it does not sweep a note that is already sounding.
- Click and drag across notes, or use **Shift+arrows** / Shift+click, to select a rectangular block. **Cmd/Ctrl+A** selects all notes. Use normal copy/cut/paste shortcuts to exchange tab-separated note blocks. Paste clips to the pattern's bounds. Delete clears the selection.
- **Block editing** (record mode on; every one is a single undo step, and columns drawn by a graphical envelope are left alone). Where Renoise has the same command the key is the same; the others use Cmd on macOS and Ctrl elsewhere.
  - **Delete** clears the selected cells. **Backspace** deletes the row at the top of the selection and pulls the rest of the track up. **Insert** (or **Cmd/Ctrl+Enter**, for keyboards without Insert) inserts a blank row there and pushes the rest down. These move notes, sub-columns and lanes together. **Cmd/Ctrl+Shift+Insert** and **Cmd/Ctrl+Shift+Delete** do the same for just the selected columns. What moves off the end of the pattern is lost, and undo brings it back.
  - **Option/Alt+↑ / ↓** transpose the selected notes by a semitone, and with **Shift** by an octave. OFF, and notes that would leave C-0 to B-8, stay as they are.
  - **Cmd/Ctrl+I** interpolates: select three or more rows and each volume, pan, delay or parameter column is filled with a straight line between its first and last selected values.
  - **Cmd/Ctrl+D** copies the selection onto the rows just below it and selects the copy, so pressing it again carries on down the pattern.
  - **Page Up / Page Down** move by a bar (four beats), **Home / End** go to the first and last row, and **F9 / F10 / F11 / F12** jump to the start and the 25%, 50% and 75% points. Shift extends the selection as it does with the arrow keys.
- **Sequence shortcuts** from the grid (the ones that change the sequence do nothing while it plays): **Cmd/Ctrl+↑ / ↓** select the previous or next sequence step, **Cmd/Ctrl+Home / End** the first or last, **Cmd/Ctrl+←  / →** point the step at the previous or next pattern, **Cmd/Ctrl+Insert** inserts a new blank pattern after it and **Cmd/Ctrl+Delete** removes the step.
- **Cmd/Ctrl+Z** undoes and **Cmd/Ctrl+Shift+Z** or **Cmd/Ctrl+Y** redoes grid edits. The toolbar buttons do the same. CodeMirror has its own per-definition undo history.
- Set tempo and rows per beat while stopped. Patterns can have different lengths; song playback transitions at their row boundaries. Edits to notes/assignments take effect beyond the scheduler's existing 200 ms lookahead.
- **Your own instruments and effects** are the **Project** folder at the top of each browser tab, numbered as the **Ins** column counts them, with a `×n` for how many tracks use each instrument or strips run each effect. The buttons under the list act on the selected one: **+** adds a blank, **⧉** duplicates it (with fresh names inside its code), **−** deletes it unless a track plays it, **{}** opens its code, and **♪** (instruments) imports WAV files: each is copied into the project's folder and becomes a sampler instrument written as ordinary code (`sample("samples/kick.wav").root(c4)`), so root note, looping and tuning are edited in the editor. The engine resolves sample paths against the project folder, so it needs to run on the same machine. Enter on an instrument chooses it, and on an effect adds it to the selected strip.
- **Sound browser.** The panel on the right stays open beside the grid. **Instruments** and **Effects** tabs list the ready-made sounds in folders, with a search box. The folders start closed, except **Project**; the arrow keys move over folders and items alike (**→** or **Enter** opens a folder, **←** closes it) and clicking a folder toggles it. Move through the sounds with the **arrow keys** (or click): each one plays as you land on it, an instrument as a note on the selected track and an effect added to that track's chain while you browse (and over the song if it is playing), without changing the project. **Instruments are slots, as in Renoise.** Landing on one of your own (the **Project** folder) makes it the *current instrument*: its code shows in the code pane, and every note you enter in record mode gets its number in the **Ins** column, which you can edit afterwards. **Enter** or double-click on a ready-made instrument adds it as a new slot and makes it current; **Shift+Enter** loads it into the current slot instead, replacing that slot's name and code (so the notes already using the slot change sound). Either way the cursor returns to the grid. **Enter** on an effect puts it on the end of the selected track's chain and opens its code. **Escape** returns to the grid. **Cmd/Ctrl+B** moves to the browser, and hides it when you are already in it.
- **Names.** An instrument or effect has one name, and its code is written to match: **Warm saw** is `instr warm_saw`. Renaming one in the code pane's name box rewrites the name in its code too (a name another definition's code already uses gets `_2`, `_3`...).
- **Ready-made instruments and effects.** The folders below Project in the browser (drums/kicks, drums/snares, synths/bass, … for instruments; filters, equalizer, echo, reverb, chorus, phaser, tremolo, compressor, overdrive, lo-fi, limiter for effects) come from the language's own library, `std/instruments` and `std/fx` (see [the standard library](../docs/src/content/docs/reference/language/standard-library.mdx)), so nimlive can `use` the same ones by name. **+ Add effect…** in a track's FX view opens the same list for effects. Adding one copies it into the project, so its code is yours to edit in place and a later change to the library does not touch your song.
- **Effects are shared code.** An effect used on several tracks, or on the master, is one piece of code: editing it and pressing **Apply** changes every device running it, while each device keeps its own slider values and LFOs.
- **Mixer and scopes.** **Mixer** in the toolbar opens a tab with a strip for every track and the master: a scope of the channel's finished signal (after its effects), pan, a fader with a level meter (double-click resets to 0 dB, arrow keys nudge it), **M** mute, **S** solo and **FX**. While any track is soloed only soloed tracks sound. A thin scope of the master runs across the top of the window. Scopes need the engine connected, and update about twenty times a second.
- **Returns, sends and outputs.** **+ Return** in the **Mixer** adds a return track (up to eight; a strip appears only once its return exists) with its own level, pan, mute and effect chain, feeding the master. A track's **output** (the dropdown on its strip) is *Master*, one of the returns, or *None*. Returns have an output and send devices of their own, but only to *later* returns (A can feed B, never the reverse), so signal can't loop; nested groups work the same way. Point several tracks at one return to make a group: kick, snare and hats into a Drums return with a compressor on it, heard only through that compressor. To send some of a track in parallel instead, add a **Send** device from the track's FX view (**+ Add effect → Send**) and choose its return on the device. By default a send is post-fader: it takes the track's finished signal after its effects, level and pan, so it follows the fader and a muted track sends nothing. Tick **Pre-fader** on the device to take the signal at the device's own place in the chain instead: the fader no longer affects it, effects before it shape what is sent, and effects after it do not. A pre-fader send can also **Mute source**, which silences everything after it in the chain so the track is heard only through its sends. Its amount is an ordinary parameter, so it has lanes, envelopes and an LFO like any other. A track with output *None* and a send is also heard only through the send. Put the effect on the return, with its mix fully wet. **×** removes a return: anything that fed it goes back to the master.
- **Returns and the master in the grid.** After the tracks the grid draws a narrow column for each return and one for the master (white), with parameter lanes only: no notes, no sub-columns. Automate a return's reverb mix, its level and pan, or the master's level, the same way as on a track: **P** on a lane picks the parameter (its **Vol**, **Pan** or one of its own effects' parameters, with no instruments), **⌁** opens an envelope, and the **⋯** menu adds or hides up to four lanes. A return column also has **M**; the master has no mute. Selecting one of these columns makes it the strip the code pane's FX view and the ↳ buttons work on, and **Tab** walks through them after the last track. Every pattern has its own lanes for each, copied into a variation.
- Each track has volume, pan, mute, solo and an **effect chain** of up to eight devices, run in order. The master has its own volume, pan and chain, which every track feeds. Click **FX** in a track header (or **FX** on the master strip in the **Mixer**) to open a chain: the **+ Add effect…** list runs an effect from the project's **Effects** library, or makes a new one from a preset or a blank. A device is a card with bypass (⏻), move (↑ ↓) and remove (×) buttons; click one to edit its code and see its sliders and LFOs. Removing a device keeps the effect in the library; **−** under **EFFECTS** deletes an effect that no device uses.
- Open a sound from the browser's Project folder, or double-click a ready-made one. Hover for compiler documentation, Ctrl+Space for completion, and Shift+Alt+F to format. Change `saw` to `sawz` to try error diagnostics.
- Click **Apply** to compile code into the engine. Rejected edits leave the last applied source sounding. Apply updates every track using an effect with a short crossfade.
- An instrument slot currently needs one instrument with a `freq: hz` parameter. An effect slot needs one `fx name { … }`, using `in` for incoming audio. Helper functions/constants may accompany either. Top-level sound names must be unique across slots.
- Edits autosave locally. **Save** writes a project folder (`name.nimtracker/` with `project.json` and a `samples/` folder) and **Open** takes its `project.json`. Source, notes, assignments, tempo, volume, mute, effect chains, LFO settings and parameter lanes are saved. Opening a project disconnects the previous session so definitions cannot leak between songs. The project format is version 9; earlier formats are not imported.

## Parameter lanes and modulation

Each track starts with a **Track volume** lane. A level in dB is shown in the cell as a bare number (`-6`, `-47.5`, to one decimal place at most); hover a cell for its unit. Press **P** with the cursor on a parameter lane to choose its parameter from the keyboard (a list with a search box: type part of a name, **Enter** takes the first match, **Escape** goes back to the grid), or use the lane’s header dropdown, to choose **Vol** or **Pan** (under *Track*), or a parameter listed under the effect or instrument it belongs to, such as *Stereo delay* › **feedback**. Connect and Apply effects to expose their parameters. **⋯** in the track header adds or hides up to four lanes per track per pattern. A lane with values must be cleared before retargeting or removing it. Each parameter can have one lane per track.

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

Not implemented yet: curved envelope interpolation, per-note modulation of a sounding note, slices and kits as notes, waveform preview, meters, per-note effect commands (parameter lanes cover most of them). Pattern undo covers note, sub-column and lane cell edits, not mixer, instrument or chain changes.

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
