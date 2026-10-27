---
title: The grid
description: Entering notes, note columns, commands, and sliced samples in the tracker.
---

The grid is where you write notes: one row per step in time, one column per track. Rows are grouped by the meter into bars; **Page Up / Page Down** move by a bar, **Home / End** go to the first and last row.

## Entering notes

Enter notes on the keyboard as a piano: the **Z** row (**Z S X D C V G B H N J M**, continuing **, L . ; /**) plays from the selected octave and the **Q** row (**Q 2 W 3 E R 5 T 6 Y 7 U**, continuing **I 9 O 0 P**) from the octave above. Move with the arrows, and clear with Delete (Backspace deletes the row instead, as in Renoise).

**Step** sets how many rows to advance after note entry (0–256): 4 at four rows per beat enters notes on consecutive beats, 16 on consecutive bars, and 0 stays on the current row. Arrows still move by one row. Octave is adjustable. Entered notes play while stopped and connected.

**Space** plays and stops when the grid is focused; **F6** switches between the grid and the instrument's code. Code editing keeps its normal shortcuts, and Space in code does not affect playback.

## Record mode

**Record mode** (**● Rec** in the toolbar, or **Escape** from anywhere) controls editing. With it off, note keys play the selected instrument from any grid column, including during playback, and typing, Delete, cut and value editing do nothing. With it on, a red frame surrounds the grid and notes and values are entered at the cursor. Corner brackets and a tinted header always mark the selected track.

The cursor's row is highlighted across every track, and the grid scrolls to keep it in the middle of the view. With record mode on, a click moves the cursor to that cell and centres its row. With it off, a click only marks cells (red) so you can copy them with **Cmd/Ctrl+C**, and drag or shift-click marks a block; the cursor stays where it is and the grid does not scroll. **Double-click** a cell to move the cursor there and centre it, then paste with **Cmd/Ctrl+V**, which works with record mode off too. Pressing a key clears the marked cells.

## How long a note lasts

Notes are held until the next note, an **OFF** or a **CUT** in the same column. Press **A** in a note column to enter an OFF, which lets the note release (an instrument with a release stage fades out), or the **minus** key to enter a CUT, which stops it at once with no release. Neither takes an instrument or makes a sound as you enter it, and both advance by the Step like a note.

There is no fixed note length, so leave a gap with OFF for staccato. A looped pattern holds a lone note for the whole pattern, and a song holds the last note until it ends.

## Note columns and sub-columns

The **⋯** button in a track header adds up to eight note columns (for chords; shared by every pattern), up to four **command columns**, and shows or hides the **Vel**, **Pan** and **Dly** sub-columns beside each note (the menu calls them **Velocity**, **Panning** and **Delay**). The **Ins** column is always there. Cells take plain numbers, edited like any other cell (click one or just type a digit, then Enter to save and move down):

| Column | What it holds |
| --- | --- |
| **Ins** | An instrument's number (shown before its name in the browser's **Project** folder), which plays that note and the notes below it in the column until another is set. |
| **Vel** | Velocity: how hard the note plays, 0–100. The track's own volume is an automation lane. |
| **Pan** | −50 (left) to +50 (right), or `C` for centre. |
| **Dly** | Starts the note that many percent of a row late, 0–99. |
| **Fx** | A [command](#commands), for the note or for the track. |

Hidden sub-columns still play.

## Commands

A command is a letter and two hex digits, as in Renoise's effect commands: type `S80`, `g10` or `R03` (letters are made capitals). The same column also takes the [track commands](/reference/tracker/automation/#track-commands) `Lxx`, `Pxx` and the four-digit `xyzz`, so a cell holds up to four characters. Each note column can have up to four command columns (each track starts with one), set under **Command columns** in **⋯**; commands in the same row combine, and a later column's wins if two of the same kind meet. Each acts on the note in its own row, or on the note held above it in the same column, until an **OFF** or **CUT** ends that hold. Whatever a command needs, the instrument has to have the parameter it sets: a start offset needs `begin`, backwards needs `rate`.

A tick is a twelfth of a row, as in Renoise. As in Renoise, `00` on `G`, `U`, `D`, `I` and `O` repeats the last value of that command above it in the column (`U10` then `U00` slides twice), and does nothing if there is none.

| Command | What it does |
| --- | --- |
| `Sxx` | Starts a new note's sample `xx`/256 of the way in: `S80` is half way (the instrument's `begin`). |
| `B00`, `B01` | Plays the new note's sample backwards (`rate` of −1) or forwards. |
| `Gxx` | Glides: instead of starting a new note, the note held in the column slides to this row's pitch at `xx` sixteenths of a semitone per tick (`G10` crosses an octave in a row; `GFF` jumps at once). The sound carries on unbroken, pitch moving evenly in semitones. |
| `Uxx`, `Dxx` | Slides the held note up or down by `xx` sixteenths of a semitone per tick for the row (`U10` is an octave). Repeat down the column for a longer slide. |
| `Rxy` | Retriggers: the note plays again every `y` ticks (1–6, so 12 to 2 hits in the row), each hit's level changed by `x`: `0` and `8` keep it, `1`–`5` lower it by 3, 6, 12, 25 and 50%, `6` and `7` by 33 and 50%, `9`–`D` raise it by 3, 6, 12, 25 and 50%, `E` and `F` by 50% and 100%. Unlike Renoise every change builds on the last hit. The last hit is held as the note would be. |
| `C0y` | Cuts the note `y` ticks (1–11) into the row, for staccato notes. Blank holds the note until the next note or **OFF**. Renoise's `Cxy` can also drop the level to `x` instead of 0; only `C0y` is supported. |
| `Yxx` | Plays the note with probability `xx`/255 (`Y01` to `YFF`). Each time the pattern plays, every note with a probability rolls afresh, so loop passes and restarts differ. A note that does not play leaves the note before it sounding, as if the cell were empty, and drops its commands with it. An export draws once for the whole song, and the mix and every stem share the same draws. |
| `Ixx`, `Oxx` | Fades the held note's level in or out by `xx` volume units (`00`–`80`, out of 128) during the row, starting from the note's **Vel**. |

A glide keeps one voice and a retrigger restarts it, so a row cannot have both, even in two command columns: entering one where the other is refused with a message, and a project that has both is rejected. A slide and a retrigger can share a row.

Hover a command to read what it does in words (`S80 starts the sample 50% of the way in`); the status line says the same as you type one and when you save it, and says why if it cannot be played.

Type into them like any other cell: click it or just start typing, **Enter** to save and move down by **Step**, **Escape** to cancel, and clearing the cell removes the command. Entering an `S`, `B` or `R` command plays the note it acts on at once, through the track's effects, so you can try a start point or a stutter without playing the pattern. Glides and slides are heard when the pattern plays.

Cells are stored as the text you typed, so they copy, paste and save as `G10`; they move with their note when a block is inserted, deleted or pasted, and undo covers them.

## Sliced samples

The **✂** button above the instrument list makes a new instrument from the current sampler's sample, cut into 4, 8, 16 or 32 equal pieces, into any number from 2 to 48 (**Pieces**, then **Go**), or **At transients**, which finds where the hits of the sample begin (up to 48, at least 60 ms apart, each cut a millisecond before the hit so the attack is whole) and starts a piece at each.

Each key plays a piece, counting up from **C-4** (C#4 the second, and so on), at its recorded pitch; keys outside the pieces are silent. Write a pattern of keys to rearrange a break, and add **Rtg** and **Start** to the pieces.

### The Sample view

The **Sample** view (the bar above the grid, or View → Sample) draws the waveform of the current sampler. For a sliced instrument it marks each cut with the key that plays the piece after it. **Click** a piece to hear it. **Drag** a marker to move a cut (the code is rewritten when you let go, and applied), **double-click** the waveform to add a cut, **double-click** a marker (or select it and press Delete) to remove it. The first cut stays at the start and a sample keeps at least two pieces. **Find hits** moves the cuts to where the hits begin, and **Cut equally** starts again with that many equal pieces. A sampler that is not cut shows its waveform too; click to hear it. A kit has no single sample to show.

**Stretch ×** changes duration while preserving the selected pitch: `2` lasts twice as long, `0.5` lasts half as long, and `1` uses ordinary playback. Choose **Granular** for fixed grain positions and a lower CPU cost, or **WSOLA** to align neighbouring waveforms for smoother joins. **Transient WSOLA** protects detected forward attacks and stretches the bodies between them. Changing either control rewrites and applies the instrument code; the choice is saved with the instrument. The range is 0.25 to 4, and sliced instruments stretch each piece. A kit exposes `stretch` and `stretch_mode` through its instrument parameters and code.

**Estimate beats** uses detected hit spacing to suggest the number of beats in the whole sample and shows the corresponding recorded BPM. It fills the beat-count field without changing the instrument, cuts, loop points or existing fit. Review it by ear, use **÷2** or **×2** if it picked half/double time, then press **Fit to tempo**. You can also edit the count manually. The estimate works best on trimmed rhythmic loops, looks initially between 60 and 200 bpm, and allows subdivisions and missing hits. Fewer than four detected positions or weak rhythmic evidence leaves the current count alone and asks for a manual value. It cannot reliably determine the intended beat level from hit spacing alone.

**Fit to tempo** plays the sample in time with the song at any tempo, keeping its pitch. Say how many beats the whole sample lasts as recorded (**lasts 4 beats**) and press **Fit to tempo**: that sets the instrument's `from_bpm` to the tempo the sample was recorded at, and every note is stretched by that tempo over the song's tempo at the note, on top of its own **Stretch ×**. A four-beat loop that lasts two seconds is a 120 bpm loop, so it fits a 100 bpm song by lasting 1.2 times as long, and it follows tempo changes, pattern tempos and ramps from note to note (a note keeps the stretch it started with). **Off** sets `from_bpm` to 0, which leaves the sample alone. The stretch is held between 0.25 and 4. In a sliced instrument each piece is stretched the same way, so a rearranged break stays in time and each piece still starts on its hit; that is a better way to fit a break than stretching it whole. A fitted instrument is stretched when it is auditioned and when MIDI plays it too.

WSOLA can reduce beating on sustained sounds. Transient WSOLA protects up to 15 ms of detected attacks; quiet or closely spaced hits can still be missed, and reverse playback uses ordinary WSOLA. If `gate` is 1, a note can end before the stretched sample does: lengthen the note, or set `gate` to 0 to hear the whole sample.

For a whole sample, choose **Forward loop** or **Ping-pong loop**, then set **Start** and **End** as percentages, or drag their markers on the waveform. The shaded region repeats after the intro has played. This edits the existing sampler rather than making slices. Turning looping on enables `gate` so OFF or the next note can end it. **Loop off** keeps the points for later. Points are saved in the instrument code. Edits also reach held playback and MIDI notes, taking effect at their next loop wrap or ping-pong turn. Start must be before End; **Crossfade** sets the overlap in milliseconds of source audio (0 turns it off). It smooths wraps and ping-pong turns, including reverse and stretched playback, and is capped at half the usable loop span. The overlap shortens each repeat; longer fades can soften attacks.

The code is ordinary (`chops = amen.slices(16)`, or `amen.slices([0, 0.18, 0.43])` for pieces that start at those shares of the sample, with a small wrapper), so use `.beats(4)` and `.choke(...)` on the sample as the [language reference](/reference/language/samples/) describes. An instrument is sliced when it has a `sample` parameter and the code cuts one sample into pieces.

## MIDI note entry

Choose a keyboard under **MIDI**, then **Ch** (All or 1–16). **↻** refreshes inputs after connecting a device.

With record mode off, keys play the selected instrument on the cursor's track; previews follow note-off and support held chords and velocity. With it on, each key press writes its absolute pitch, instrument and hardware velocity into the current note column and advances by **Step**. A sub-column or parameter lane targets its track's note column; returns and the master use the last musical track. During playback, notes go to the nearest row without stepping. Note-off releases the preview; recorded note lengths follow the usual note/OFF rules. Pitches C-0 through B-8 are supported. Device and channel changes, unplugging, window blur and Stop release previews. MIDI is off until an input is selected, and the input selection is separate from the saved song.
