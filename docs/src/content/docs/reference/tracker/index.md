---
title: niminal tracker
description: A desktop tracker for niminal — pattern grid, mixer and a code pane, connected straight to the audio server.
---

**Compiled by niminal.** The instrument and effect code you edit in the tracker is the same language as the server's half of a `.nml` file, compiled by the server as you press **Apply**. The notes, patterns, tracks and automation belong to the tracker's own project, not to a `.nml` file.

The tracker is a desktop app: a keyboard-driven pattern grid, a mixer, and an editor for the code behind each sound. It connects directly to `niminal` and does not need `nimlive`, so the only program you need installed beside it is the server.

## Open it

The installed macOS release opens with:

```sh
open "$HOME/Applications/niminal tracker.app"
```

Elsewhere, build it from [`tracker/`](https://github.com/martineastwood/niminal/tree/main/tracker) in the repository (it needs the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/), Node.js and Rust).

## Connecting to the server

The tracker starts the engine itself: when it opens it connects to `niminal server` on port 7401, and if nothing is listening it runs the `niminal` program it finds on the search path or in `~/.cargo/bin`, and stops it again when it closes. **Connect** in the footer reconnects. An engine you started yourself is used as it is and left running.

Click **▶ Pattern** to hear the current pattern. Entering a note in the grid plays it on the current instrument, through the track's effects, even while stopped.

## The window

The **pattern grid** is the main workspace. A single toolbar holds transport, tempo (BPM, LPB), octave, entry step, pattern and track controls; engine connection and status live in the footer.

The bottom dock has **Code**, **Automation**, **Mixer**, **Matrix** and **Sample** tabs. It starts open on **Code**, showing the current instrument. **Instrument** / **FX** at the top of the pane switch it between the instrument's code and the selected track's effect chain, with presets and generated controls beside the code. **×** closes the dock, **⤢** expands it over the grid, and dragging its top edge resizes it. **F6** switches between the grid and the editor.

The right-hand side holds the project's instruments and the sound browser. **Cmd/Ctrl+B** shows or hides the browser.

The **menu bar** has File (New, Open, Save, Import Samples, Import Samples as a Kit, Export WAV, Export WAV and Track Stems), Edit (Undo, Redo, Cut, Copy, Paste), Song, Sound and View. Items do what the matching button or key does.

- [The grid](/reference/tracker/grid/) — notes, columns, start, pitch and retrigger cells, sliced samples
- [Patterns, the sequence and timing](/reference/tracker/sequence/) — song order, loops, sections, the matrix, tempo
- [Instruments and effects](/reference/tracker/instruments/) — the project's sounds, the browser, applying code, effect chains
- [Parameter lanes, LFOs and envelopes](/reference/tracker/automation/) — automation inside a pattern
- [Mixer, returns and sends](/reference/tracker/mixer/) — levels, groups, parallel effects
- [Selecting and editing](/reference/tracker/editing/) — block operations, shortcuts, undo, MIDI

## Saving and opening

Edits autosave locally. **Save** writes a project folder (`name.nimtracker/` with `project.json` and a `samples/` folder) and **Open** takes its `project.json`. Source, notes, assignments, tempo, volume, mute, effect chains, LFO settings and parameter lanes are saved. Opening a project disconnects the previous session so definitions cannot leak between songs. Earlier project formats are not imported, so a project saved by an older version has to be rebuilt.

## Exporting audio

**File → Export WAV…** renders the whole song from its first step to a 32-bit float stereo WAV file, and **Export WAV and Track Stems…** also writes one file per unmuted track and return (`name-01-Lead.wav`, post-fader, before the master; muted tracks, and tracks silenced by a solo, are left out). Export stops tracker playback first, then renders as fast as the engine can compute it. The loop is ignored, the tail of the last notes and effects is kept until it falls quiet (up to a minute), and the mix uses the song's routing, automation and last applied sounds. The engine must be running on this machine.

## Not there yet

Curved envelope interpolation, and automatic loop-point selection.
