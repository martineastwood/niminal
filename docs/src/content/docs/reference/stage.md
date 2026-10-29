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

## Views and files

| Key | Does |
| --- | --- |
| Shift+Cmd/Ctrl+L | Browse files |
| Cmd/Ctrl+B | Show or hide the sidebar |
| Cmd/Ctrl+J | Show or hide the controls |
| Shift+Cmd/Ctrl+P | Performance mode: just the code and what is playing |
| Cmd/Ctrl + and − | Text size (or pinch) |
| Cmd/Ctrl+O, Cmd/Ctrl+S | Open and save a file |
| Shift+Cmd/Ctrl+O | Open a project folder |

Open tabs and their text are remembered between runs.
