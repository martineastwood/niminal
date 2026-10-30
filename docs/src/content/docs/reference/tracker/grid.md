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

The cursor's row is highlighted across every track, and the grid scrolls to keep it in the middle of the view. A click moves the cursor to that cell in either mode, and dragging or shift-clicking selects a block. With record mode on, a click on a command, velocity or other value cell also starts editing it. The arrows stop at the edges of the pattern and the columns rather than wrapping round.

**Follow** (in the toolbar) decides what the grid does while the song plays. Ticked, the pattern changes with the song and the cursor is the play position: it moves down the pattern with the music, the grid scrolls beneath it, and when you stop it stays on the row where playback ended. In record mode notes land on the nearest row as it passes. Unticked, the cursor stays where you put it: move around and edit freely while the music loops.

**Space** plays the pattern from its first row and **Shift+Space** from the cursor's row. Both loop the pattern.

## Moving and copying cells

In record mode, drag a block of selected cells to move it: grab any cell inside the selection and drop it where you want it, and the notes and values go with it, leaving blanks behind. Hold **Alt/Option** as you let go to copy instead. The block stays inside the pattern and over columns of the same kind, so a note never lands in a velocity column, and one undo step reverses the move. A plain click inside a selection just puts the cursor there.

**Right-click** a cell for a menu of what you can do with the selection (copy, paste, clear, transpose, interpolate, repeat below, reverse, randomize), with the keys beside each item. Everything but copy needs record mode on, and says so if it is off.

## How long a note lasts

Notes are held until the next note, an **OFF** or a **CUT** in the same column. Press **A** in a note column to enter an OFF, which lets the note release (an instrument with a release stage fades out), or the **minus** key to enter a CUT, which stops it at once with no release. Neither takes an instrument or makes a sound as you enter it, and both advance by the Step like a note.

A faint line (│) runs down the empty rows of a note column for as long as its note sounds, up to the next note, OFF or CUT, so you can see where notes end and where they run on. There is no fixed note length, so leave a gap with OFF for staccato. A looped pattern holds a lone note for the whole pattern, and a song holds the last note until it ends.

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

Cutting is done in the [Sample view](#the-sample-view), in one place: **Find hits** makes a new instrument from the current sampler's sample cut where its hits begin (up to 48, at least 60 ms apart, each cut a millisecond before the hit so the attack is whole), and **Cut equally** cuts it into any number of equal pieces from 2 to 48. Neither touches the sampler itself.

Each key plays a piece, counting up from **C-4** (C#4 the second, and so on), at its recorded pitch; keys outside the pieces are silent. Write a pattern of keys to rearrange a break, and add **Rtg** and **Start** to the pieces.

### The Sample view

The waveform fills the view, with a ruler of seconds along its bottom edge. Above it are **Find hits** (with its sensitivity in %, 50 the usual: higher finds softer hits and more of them) and **Cut equally**. The settings are a column on the right, a label and a value on each row: **Plays**, the loop's **Loop start**, **Loop end** and **Crossfade** (shown only while the sampler is a loop), **Root note** and **Finetune** with **Match tempo**, **Fit to tempo**, **Lasts … beats** and **Fit by**, **Stretch ×**, **Algorithm**, **Release** and **Interpolation**. **Root note** and **Finetune** (up to ±50 cents) set the note at which the sample plays as recorded, which is `.root(...)` in its code: a plain note is written as a note (`.root(a3)`), anything with cents as a frequency. Lowering the root plays every note higher, and so faster. **Match tempo** does that once: it retunes the root so the sample lasts its **Lasts … beats** at the song's tempo now (the beat count is worked out from the hits if you have not entered one). It changes the pitch and does not follow later tempo changes, which **Fit to tempo** does, so it asks you to untick that first. **Fit by** appears while the sample is fitted: **Stretch** keeps the pitch as the tempo changes, **Repitch** plays faster or slower, as tape would, and the pitch follows (it changes the note's `rate`, so **Stretch ×** still applies on top). **Release** is how long the sample takes to fade out when its note ends, which softens the end of a held sample or loop. **Interpolation** chooses how the sample is read between its frames at another pitch: Linear (`.interp(linear)`, the lightest and roughest at low pitches), Cubic (the default), or Sinc (`.interp(sinc)`), which aliases least and costs the most CPU.

Where cuts are close together the waveform names only some of them, so the labels stay readable; the selected and hovered cut always show theirs.

The **Sample** view (the bar above the grid, or View → Sample) draws the waveform of the current sampler. For a sliced instrument it marks each cut with the key that plays the piece after it. **Click** a piece to hear it. **Drag** a marker to move a cut (the code is rewritten when you let go, and applied), **double-click** the waveform to add a cut, **double-click** a marker (or select it and press Delete) to remove it. The first cut stays at the start and a sample keeps at least two pieces. **Find hits** moves the cuts to where the hits begin, and **Cut equally** starts again with that many equal pieces. On a sampler that is not cut yet, **Find hits** and **Cut equally** make a new instrument that plays its sample in those pieces (the original is left as it was), and the Sample view shows the new one, ready to adjust. Click the waveform of an uncut sampler to hear it. A kit has no single sample to show.

**Stretch ×** changes duration while preserving the selected pitch: `2` lasts twice as long, `0.5` lasts half as long, and `1` uses ordinary playback. Choose **Granular** for fixed grain positions and a lower CPU cost, or **WSOLA** to align neighbouring waveforms for smoother joins. **Transient WSOLA** protects detected forward attacks and stretches the bodies between them. Changing either control rewrites and applies the instrument code; the choice is saved with the instrument. The range is 0.25 to 4, and sliced instruments stretch each piece. A kit exposes `stretch` and `stretch_mode` through its instrument parameters and code.

**Fit to tempo** plays the sample in time with the song at any tempo, keeping its pitch. The first time you press **Fit to tempo**, the number of beats the whole sample lasts is worked out from the hits in it (if they are not regular enough, you are asked to type it in **lasts … beats** and press again). If it sounds half or double speed, halve or double that number: once the sample is fitted, changing it refits at once. Fitting sets the instrument's `from_bpm` to the tempo the sample was recorded at, and every note is stretched by that tempo over the song's tempo at the note, on top of its own **Stretch ×**. A four-beat loop that lasts two seconds is a 120 bpm loop, so it fits a 100 bpm song by lasting 1.2 times as long, and it follows tempo changes, pattern tempos and ramps from note to note (a note keeps the stretch it started with). Unticking **Fit to tempo** sets `from_bpm` to 0, which leaves the sample alone, and remembers the beat count for the next fit. The stretch is held between 0.25 and 4. In a sliced instrument each piece is stretched the same way, so a rearranged break stays in time and each piece still starts on its hit; that is a better way to fit a break than stretching it whole. A fitted instrument is stretched when it is auditioned and when MIDI plays it too.

WSOLA can reduce beating on sustained sounds. Transient WSOLA protects up to 15 ms of detected attacks; quiet or closely spaced hits can still be missed, and reverse playback uses ordinary WSOLA. If `gate` is 1, a note can end before the stretched sample does: lengthen the note, or set `gate` to 0 to hear the whole sample.

The **Plays** choice at the top of the Sample tab says what a sampler does when its note ends: **One-shot** plays to the end and ignores OFF and CUT, **Hold** plays while the note is held and ends with it, and **Loop** and **Ping-pong** repeat a region while it is held. A kit can be a one-shot or held, but not looped. A cut instrument can be looped too, but a loop repeats the whole sample, so choosing **Loop** or **Ping-pong** asks first and then turns it back into a plain sampler of its whole sample: its pieces go, and notes written for them (C-4, C#4, …) will each play the whole sample from its start. **Play as one sample**, in the bar above the waveform while the instrument is cut, does the same without choosing a loop. It rewrites the sampler's `gate` and `.loop(...)`, so the code stays the thing you can read and edit. For a whole sample, choosing **Loop** or **Ping-pong** lets you set **Start** and **End** as percentages, or drag their markers on the waveform. The shaded region repeats after the intro has played. This edits the existing sampler rather than making slices. Looping enables `gate` so OFF or the next note can end it, and switching to **One-shot** or **Hold** keeps the points for later. Points are saved in the instrument code. Edits also reach held playback and MIDI notes, taking effect at their next loop wrap or ping-pong turn. Start must be before End; **Crossfade** sets the overlap in milliseconds of source audio (0 turns it off). It smooths wraps and ping-pong turns, including reverse and stretched playback, and is capped at half the usable loop span. The overlap shortens each repeat; longer fades can soften attacks.

The code is ordinary (`chops = amen.slices(16)`, or `amen.slices([0, 0.18, 0.43])` for pieces that start at those shares of the sample, with a small wrapper), so use `.beats(4)` and `.choke(...)` on the sample as the [language reference](/reference/language/samples/) describes. An instrument is sliced when it has a `sample` parameter and the code cuts one sample into pieces.

## MIDI note entry

Choose a keyboard under **MIDI**, then **Ch** (All or 1–16). **↻** refreshes inputs after connecting a device.

With record mode off, keys play the selected instrument on the cursor's track, and a note sounds for as long as its key is held and stops when the key comes up (so a loop or a held sample plays right through, a one-shot plays out whatever you do, and a chord of keys holds each note on its own; the same goes for the note you type in record mode and for keys pressed outside the grid); previews follow note-off and support held chords and velocity. With it on, each key press writes its absolute pitch, instrument and hardware velocity into the current note column and advances by **Step**. A sub-column or parameter lane targets its track's note column; returns and the master use the last musical track. During playback, notes go to the nearest row without stepping. Note-off releases the preview; recorded note lengths follow the usual note/OFF rules. Pitches C-0 through B-8 are supported. Device and channel changes, unplugging, window blur and Stop release previews. MIDI is off until an input is selected, and the input selection is separate from the saved song.
