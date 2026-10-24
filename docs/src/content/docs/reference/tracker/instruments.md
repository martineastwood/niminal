---
title: Instruments and effects
description: The project's sounds, the sound browser, applying code, and effect chains in the tracker.
---

## The project's instruments

A panel at the top of the right-hand side holds the project's instruments, always in view and scrolling when there are many: numbered as the **Ins** column counts them, with a `×n` for how many tracks use each, and the current one highlighted.

**Option+↑ / ↓** (from anywhere except a text box) moves to the previous or next instrument and plays a note, so you can pick a sound without the mouse; clicking one, or ↑ / ↓ with the list focused, does the same, **Enter** or a double-click opens its code, and **Escape** returns to the grid.

The buttons above the list add a blank (**+**), duplicate (**⧉**) or delete (**−**) the current one, import WAV files (**♪**), make a kit (**▦**) and cut a sampler into pieces (**✂**, see [Sliced samples](/reference/tracker/grid/#sliced-samples)). Each imported WAV is copied into the project's folder and becomes a sampler instrument written as ordinary code (`sample("samples/kick.wav").root(c4)`), so root note, looping and tuning are edited in the editor. The engine resolves sample paths against the project folder, so it needs to run on the same machine.

A **Project** folder at the top of the browser's **Effects** tab lists the project's own effects in the same way, with buttons under the list.

## The sound browser

The panel on the right stays open beside the grid. **Instruments** and **Effects** tabs list the ready-made sounds in folders, with a search box. The folders start closed, except **Project** on the Effects tab; the arrow keys move over folders and items alike (**→** or **Enter** opens a folder, **←** closes it) and clicking a folder toggles it.

Move through the sounds with the **arrow keys** (or click): each one plays as you land on it, an instrument as a note on the selected track and an effect added to that track's chain while you browse (and over the song if it is playing), without changing the project.

**Instruments are slots, as in Renoise.** The *current instrument* is the one chosen in the project panel: its code shows in the code pane, and every note you enter in record mode gets its number in the **Ins** column, which you can edit afterwards. **Enter** or double-click on a ready-made instrument adds it as a new slot and makes it current; **Shift+Enter** loads it into the current slot instead, replacing that slot's name and code (so the notes already using the slot change sound). Either way the cursor returns to the grid. **Enter** on an effect puts it on the end of the selected track's chain and opens its code. **Escape** returns to the grid. **Cmd/Ctrl+B** moves to the browser, and hides it when you are already in it.

## Names

An instrument or effect has one name, and its code is written to match: **Warm saw** is `instr warm_saw`. Renaming one in the code pane's name box rewrites the name in its code too (a name another definition's code already uses gets `_2`, `_3`...).

## Ready-made instruments and effects

The folders below Project in the browser (drums/kicks, drums/snares, synths/bass, … for instruments; filters, equalizer, echo, reverb, chorus, phaser, tremolo, compressor, overdrive, lo-fi, limiter for effects) come from the language's own library, `std/instruments` and `std/fx` (see [the standard library](/reference/language/standard-library/)), so nimlive can `use` the same ones by name.

**+ Add effect…** in a track's FX view opens the same list for effects. Adding one copies it into the project, so its code is yours to edit in place and a later change to the library does not touch your song.

## Applying code

Open an instrument from the project instrument panel, an effect from the browser's Project folder, or double-click a ready-made sound. Hover for compiler documentation, Ctrl+Space for completion, and Shift+Alt+F to format. Change `saw` to `sawz` to try error diagnostics.

Click **Apply** (or **Cmd/Ctrl+Enter** in the editor, which also plays a note through it) to compile code into the engine. Rejected edits leave the last applied source sounding. Apply updates every track using an effect with a short crossfade.

## What a slot holds

An instrument slot currently needs one instrument with a `freq: hz` parameter. An effect slot needs one `fx name { … }`, using `in` for incoming audio. Helper functions and constants may accompany either. Top-level sound names must be unique across slots.

## Effect chains

Each track has volume, pan, mute, solo and an **effect chain** of up to eight devices, run in order. Returns and the master have their own volume, pan and chains; see [mixer, returns and sends](/reference/tracker/mixer/).

Click **FX** in a track header (or **FX** on the master strip in the **Mixer**) to open a chain. **+ Add effect…** runs an effect from the project's **Effects** library, or makes a new one from a preset or a blank. A device is a card with bypass (⏻), move (↑ ↓) and remove (×) buttons; click one to edit its code and see its sliders and LFOs. Removing a device keeps the effect in the library; **−** under **EFFECTS** deletes an effect that no device uses.

## Effects are shared code

An effect used on several tracks, or on the master, is one piece of code: editing it and pressing **Apply** changes every device running it, while each device keeps its own slider values and LFOs.

## What a note's end does to a sample

A sampler made by the tracker has a `gate` parameter and a `release`. With `gate` at 0 the sample plays out whatever the note, which suits a drum hit. With `gate` at 1 the sample ends, fading over `release` (10 ms by default), where its note does: at an OFF or CUT, or when the next note starts in the same column. That is what a loop, pad or held vocal needs, and it makes OFF work on a sample. Samples of two seconds or longer start with `gate` at 1 and shorter ones at 0; pieces of a sliced sample start at 1, and a kit at 0. Change it with its slider, a parameter lane, or the default in the code.

## Kits

**▦** (or File → Import Samples as a Kit…) asks for several WAV files and makes **one** instrument that plays them: they are copied into a folder of the project (`samples/<name>`, named for the folder they came from) and each file gets a key, counting up from **C-4** in the order of their names. The code lists the keys (`/// C-4 hat · C#4 kick · D-4 snare`), uses the language's `kit("samples/drums")`, and takes the same `begin`, `rate`, `gate` and `release` as any sampler, so **O**, **V** and **R** work on its hits. A kit holds 1 to 48 samples; sample names may use letters, digits and underscores, and a name that starts with a digit gets an `s` in front. Rename a file before importing to change which key it gets.
