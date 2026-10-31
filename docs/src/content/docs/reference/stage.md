---
title: niminal stage
description: A desktop app for live coding — code on one side, scenes, lanes and meters on the other.
---

**Read by niminal-live.** Stage is a desktop app for performing a `.nml` file. It has the live sequencer and the language server built in, so a running `niminal server` is the only other program it needs.

You write code in the editor, evaluate it, and watch the same music in the lanes beside it: the scenes you can launch, the clips that are playing, and each note lighting up in the code as it sounds.

## Open it

Stage is built from [`stage/`](https://github.com/martineastwood/niminal/tree/main/stage) in the repository (it needs the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/), Node.js and Rust):

```sh
cd stage
npm ci
npm run tauri -- dev
```

## Connecting to the server

Stage does not start an engine; run `niminal server` yourself. Stage connects to port 7401 (change it with **port** in the top bar, and give a **token** if the server was started with `--token`), and the connection light in the top left shows whether it is up. **Connect** tries again.

Stage opens with a short demo piece that only defines things. Launch its scenes with the buttons above the lanes or with Cmd/Ctrl+1…9, or open your own file.

## The window

- **Top bar.** Bar and beat, tempo and meter, the output level, an engine health light (it shows when the engine is dropping out), the number of voices, and how many changes are waiting to land. **Hush**, **Panic** and **Rec** live here too.
- **Code.** Tabs of `.nml` files, with highlighting, diagnostics as you type, completion and hover from the language server. Evaluated lines flash, and a line you sent to land later stays marked until it does.
- **Stage.** Scene buttons, then a lane for each playing clip with a playhead and the notes lit as they sound. Below them are the changes waiting to land, which you can cancel before they do.
- **Sidebar.** **Browse** opens a file tree: open pieces, hear samples before using them, and write a sample into the code as a binding.
- **Controls.** The file's named controls (`ctl`) as sliders you can move while it plays (Cmd/Ctrl+J).

## Evaluating

| Key | Does |
| --- | --- |
| Cmd/Ctrl+Enter | Evaluate the selection, or the block the cursor is in (up to the nearest blank line, widened to balance brackets) |
| Shift+Cmd/Ctrl+Enter | Evaluate the whole file |
| Cmd/Ctrl+1…9 | Start or stop the Nth scene |
| Cmd/Ctrl+. | Hush: stop every clip and let what is sounding ring out |
| Shift+Cmd/Ctrl+. | Panic: silence everything now, tails included |

The **land** box above the editor says when an evaluation takes effect, as in `now` or `next 4bars`. Leave it empty for the sequencer's default. See [When changes land](/guide/live-coding/#when-changes-land).

### Numbers

Hold **Alt** and drag a number in the code sideways to change it; the block it is in is sent when you let go (Shift moves in tens, Esc puts the number back). Cmd/Ctrl+Alt+↑ or ↓ with the cursor in a number nudges it and leaves the sending to you.

## Hardware and clocks

- **MIDI.** A MIDI chip in the top bar shows the inputs Stage found; click it to look again. A control written `ctl x = midi.learn` binds to the next control change that arrives, and Stage rewrites the line to `midi.cc(...)`. See [MIDI and OSC](/guide/live-coding/#midi-and-osc-from-a-controller).
- **Sync.** **follow** takes tempo and the bar line from an incoming MIDI clock; **send** sends a clock, with start and stop, to an output. See [Syncing to other gear](/guide/live-coding/#syncing-to-other-gear-with-midi-clock).

## Takes

**Rec** marks the start of a take; press it again to end the take. Then:

- **Render WAV** plays the server's session log over that stretch to a WAV file. It uses the `niminal` program, so it must be installed, and the server must have been writing a log (the default).
- **Save script** writes everything you evaluated during the take, in order, with when.

## Lab

**Lab** (or Shift+Cmd/Ctrl+B) replaces the clips and scenes strip with a place to try one instrument. Evaluate the file, press **Refresh**, and pick an instrument that has a `freq` parameter. Hold the on-screen keys, or the computer keys A W S E D F T G Y H U J K (Z and X move the octave), and **Hold** keeps a note sounding while you work.

Every parameter written with a range gets a fader, and it moves the sounding note as you drag:

```nml
instr cello(freq: hz, density: 5hz..80hz = 30hz, grain: 40ms..1sec = 500ms, room: 0..1 = 0.5) { ... }
```

Frequencies and times move in even ratios, and levels in db. Double-click a fader to return it to its default. `examples/grain_cello.nml` is a good one to start with. The Lab plays through a clip called `__lab`, which the clip list hides.

### Gallery

The **Gallery** tab makes many versions of one sample. Choose a sample, tick the families you want, pick how many (few, enough or lots) and press **Make**. Each version is a tile coloured by family: click it to hear it, click again to stop, and press **Open** to put its code in a tab. **Remix** makes a new set from the same sample. The versions are written to `~/.niminal/gallery/<sample name>`, and need the `niminal-live` program installed (see [the command line](/reference/niminal-live/cli/#gallery)).

## Views and files

| Key | Does |
| --- | --- |
| Shift+Cmd/Ctrl+L | Browse files |
| Cmd/Ctrl+B | Show or hide the sidebar |
| Shift+Cmd/Ctrl+B | The Lab: play an instrument and move its parameters |
| Shift+Cmd/Ctrl+P | Performance mode: just the code and what is playing |
| Cmd/Ctrl + and − | Text size (or pinch) |
| Cmd/Ctrl+O, Cmd/Ctrl+S | Open and save a file |
| Shift+Cmd/Ctrl+O | Open a project folder |

Open tabs and their text are remembered between runs.
