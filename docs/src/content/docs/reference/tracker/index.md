---
title: niminal tracker
description: A desktop tracker for niminal — pattern grid, mixer and a code pane, connected straight to the audio server.
---

**Compiled by niminal.** The instrument and effect code you edit in the tracker is the same language as the server's half of a `.nml` file, compiled by the server as you press **Apply**. The notes, patterns, tracks and automation belong to the tracker's own project, not to a `.nml` file.

## What is a tracker?

A tracker is a way of writing music as a spreadsheet. Time runs down the page in rows, and each column is a track, with an instrument and its own effects. You type a note into a cell (`C-4`, `E-4`, `G-4`), and when you press play a cursor steps down the rows and plays whatever it reaches. A block of rows is a pattern, and a song is a list of patterns played in order.

The style comes from home computers and the demoscene of the late 1980s, and it is still used because it is fast and exact. You work from the keyboard, you see every note and every row at once, and you can't nudge a note off the grid by accident. Where [niminal's text files](/reference/language/) describe music in code, a tracker describes it in a grid.

The tracker is a desktop app: a keyboard-driven pattern grid, a mixer, and an editor for the code behind each sound. It connects directly to `niminal` and does not need `niminal-live`, so the only program you need installed beside it is the server.

## Open it

The installed macOS release opens with:

```sh
open "$HOME/Applications/niminal tracker.app"
```

Elsewhere, build it from [`tracker/`](https://github.com/martineastwood/niminal/tree/main/tracker) in the repository (it needs the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/), Node.js and Rust).

## Connecting to the server

The tracker does not start an engine; run `niminal server` yourself. When it opens, and every few seconds after, the tracker looks for one on port 7401 and connects as soon as it is listening, so the order you start them in doesn't matter. **Connect** in the footer reconnects. Closing the tracker leaves the engine running.

Click **▶ Pattern** to hear the current pattern. Entering a note in the grid plays it on the current instrument, through the track's effects, even while stopped.

## The window

The **pattern grid** is the main workspace. A single toolbar holds transport, tempo (BPM, LPB), octave, entry step, pattern and track controls; engine connection and status live in the footer.

The bar above the grid switches the main area between **Pattern**, **Instrument**, **Sample** and **Mixer**. The Instrument view holds the current instrument's controls and code, which every track shares; Sample draws its waveform; Mixer shows a strip for every track. **Pattern** returns to the grid.

The bottom dock belongs to the selected track and stays open under every view. It is named after the track and has **FX** and **Automation** tabs. Selecting another track (in the grid, or on a mixer strip) shows that track's effect chain, with presets and generated controls beside the code of the selected effect, or its automation if that tab is open. **×** closes the dock, **⤢** expands it over the main area, and dragging its top edge resizes it. **F6** switches between the grid and the instrument's code.

The right-hand side holds the project's instruments and the sound browser. **Cmd/Ctrl+B** shows or hides the browser.

The **menu bar** has File (New, Open, Save, Import Samples, Import Samples as a Kit, Export WAV, Export WAV and Track Stems), Edit (Undo, Redo, Cut, Copy, Paste), Song, Sound and View. Items do what the matching button or key does.

- [The grid](/reference/tracker/grid/) — notes, columns, commands, sliced samples
- [Patterns, the sequence and timing](/reference/tracker/sequence/) — song order, loops, sections, the track cells, tempo
- [Instruments and effects](/reference/tracker/instruments/) — the project's sounds, the browser, applying code, effect chains
- [Track commands, automation and envelopes](/reference/tracker/automation/) — automation inside a pattern
- [Mixer, returns and sends](/reference/tracker/mixer/) — levels, groups, parallel effects
- [Selecting and editing](/reference/tracker/editing/) — block operations, shortcuts, undo, MIDI

## Saving and opening

Edits autosave locally. **Save** writes a project folder (`name.nimtracker/` with `project.json` and a `samples/` folder) and **Open** takes its `project.json`. Source, notes, assignments, tempo, volume, mute, effect chains, LFO settings and parameter lanes are saved. Opening a project disconnects the previous session so definitions cannot leak between songs. Earlier project formats are not imported, so a project saved by an older version has to be rebuilt.

## Exporting audio

**File → Export WAV…** renders the whole song from its first step to a 32-bit float stereo WAV file, and **Export WAV and Track Stems…** also writes one file per unmuted track and return (`name-01-Lead.wav`, post-fader, before the master; muted tracks, and tracks silenced by a solo, are left out). Export stops tracker playback first, then renders as fast as the engine can compute it. The loop is ignored, the tail of the last notes and effects is kept until it falls quiet (up to a minute), and the mix uses the song's routing, automation and last applied sounds. The engine must be running on this machine.

## Not there yet

Curved envelope interpolation, and automatic loop-point selection.
