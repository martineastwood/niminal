---
title: niminal stage
description: A desktop app for live coding — code on one side, scenes, lanes and meters on the other.
---

**Read by Niminal Stage.** Stage is a desktop app for performing a `.nml` file. It has the audio engine, the live sequencer and the language server built in, so it needs nothing else running.

You write code in the editor, evaluate it, and watch the same music in the lanes beside it: the scenes you can launch, the clips that are playing, and each note lighting up in the code as it sounds.

## Open it

Stage is built from [`stage/`](https://github.com/martineastwood/niminal/tree/main/stage) in the repository (it needs the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/), Node.js and Rust):

```sh
cd stage
npm ci
npm run tauri -- dev
```

## The engine, the sound card and other tools

The audio engine runs inside Stage and starts with it, so there is nothing to start first. If it could not start (no sound card, say) the top left says **engine offline**, the lanes say why, and Stage tries again every few seconds; **Restart** tries now.

**Settings** (Cmd/Ctrl+, or the menu bar: niminal stage → Settings on a Mac, File → Settings elsewhere) opens a page with an **Audio** section. It chooses the output device, the sample rate (only the rates the card supports) and the buffer size (smaller is lower latency and less forgiving). Applying them starts the engine again on them, so what is playing stops and your clips and scenes are cleared; evaluate your code again afterwards. If the new settings don't work, the previous ones are put back and the message says why. The choice is remembered. The section also has an **OSC in port**: a UDP port on this machine that controls bound with `osc_in` listen to.

Other tools reach Stage on its **tools port** (7400 by default, with an optional **token**, set in the top bar and applied with **Apply**). The VS Code extension plays through Stage there, and other sequencers reach the engine on the next port up (7401). If a port is taken, for example by a `niminal server`, the top bar says so; choose another and press **Apply**.

Stage opens with a short demo piece that only defines things. Launch its scenes with the buttons above the lanes or with Cmd/Ctrl+1…9, or open your own file.

## The window

- **Top bar.** Bar and beat, tempo and meter, the output level, an engine health light (it shows when the engine is dropping out), the number of voices, and how many changes are waiting to land. **Hush**, **Panic** and **Replay** live here too.
- **Code.** Tabs of `.nml` files, with highlighting, diagnostics as you type, completion and hover from the language server. Evaluated lines flash, and a line you sent to land later stays marked until it does.
- **Stage.** Scene buttons, then a lane for each playing clip with a playhead and the notes lit as they sound. Below them are the changes waiting to land, which you can cancel before they do.
- **Samples.** Drag an audio file from the Finder onto the code and Stage writes `sample("path")` where you let go, with the path relative to the file you are editing. On an empty line it writes a whole binding such as `kick = sample("drums/kick.wav")`; drop several files and each gets a line of its own. Dropping a `.nml` file opens it in a tab.
- **Controls.** The file's named controls (`ctl`) as sliders you can move while it plays (Cmd/Ctrl+J).

## Evaluating

| Key | Does |
| --- | --- |
| Cmd/Ctrl+Enter | Evaluate the selection, or the statement the cursor is in (one line like `melody.start`, or a whole `instr { ... }` from any line inside it) |
| Shift+Enter | Evaluate like Cmd/Ctrl+Enter, then move the cursor to the start of the next statement |
| Alt+Enter | Evaluate the lines around the cursor, up to the nearest blank lines (widened to balance brackets) |
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

## Replays

Stage logs every session as a list of timestamped commands, not as audio. **Replay** marks the start of a stretch; press it again to end it. Nothing is captured as it happens. Then:

- **Render WAV** plays the engine's session log over that stretch to a WAV file, at the sample rate the engine ran at.
- **Save script** writes everything you evaluated during the replay, in order, with when.

## Views and files

| Key | Does |
| --- | --- |
| Cmd/Ctrl+B | Show or hide the sidebar |
| Cmd/Ctrl + and − | Text size (or pinch) |
| Cmd/Ctrl+O, Cmd/Ctrl+S | Open and save a file |
| Shift+Cmd/Ctrl+O | Open a project folder |

Open tabs and their text are remembered between runs.

## Settings

**Settings** (Cmd/Ctrl+,) has an **Audio** section (above) and a **Look** section for how lit notes glow while a clip plays. Pick a preset (calm is light on the CPU, video is the strongest for screen recordings) or change a single setting: how far the glow spreads, how long a note stays lit, whether the name of the instrument a clip plays lights up where it is defined, the frame rate for lit notes and playheads (30 or 60), and a large transport readout in the top bar.
