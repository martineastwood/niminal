---
title: The grid
description: Entering notes, note columns, start, pitch and retrigger cells, and sliced samples in the tracker.
---

The grid is where you write notes: one row per step in time, one column per track. Rows are grouped by the meter into bars; **Page Up / Page Down** move by a bar, **Home / End** go to the first and last row.

## Entering notes

Enter notes on the keyboard as a piano: the **Z** row (**Z S X D C V G B H N J M**, continuing **, L . ; /**) plays from the selected octave and the **Q** row (**Q 2 W 3 E R 5 T 6 Y 7 U**, continuing **I 9 O 0 P**) from the octave above. Move with the arrows, and clear with Delete (Backspace deletes the row instead, as in Renoise).

**Step** sets how many rows to advance after note entry (0–256): 4 at four rows per beat enters notes on consecutive beats, 16 on consecutive bars, and 0 stays on the current row. Arrows still move by one row. Octave is adjustable. Entered notes play while stopped and connected.

**Space** plays and stops when the grid is focused; **F6** switches between grid and code. Code editing keeps its normal shortcuts, and Space in code does not affect playback.

## Record mode

**Record mode** (**● Rec** in the toolbar, or **Escape** from anywhere) controls editing. With it off, note keys play the selected instrument from any grid column, including during playback, and typing, Delete, cut, paste and value editing do nothing. With it on, a red frame surrounds the grid and notes and values are entered at the cursor. Corner brackets and a tinted header always mark the selected track.

## How long a note lasts

Notes are held until the next note, an **OFF** or a **CUT** in the same column. Press **A** in a note column to enter an OFF, which lets the note release (an instrument with a release stage fades out), or the **minus** key to enter a CUT, which stops it at once with no release. Neither takes an instrument or makes a sound as you enter it, and both advance by the Step like a note.

There is no fixed note length, so leave a gap with OFF for staccato. A looped pattern holds a lone note for the whole pattern, and a song holds the last note until it ends.

## Note columns and sub-columns

The **⋯** button in a track header adds up to eight note columns (for chords; shared by every pattern) and shows or hides the **Vol**, **Pan**, **Dly**, **Start**, **Chance**, **Pitch** and **Rtg** sub-columns beside each note (the menu calls them **Volume**, **Panning**, **Delay**, **Start**, **Chance**, **Pitch** and **Retrigger**).

The **Ins** column is always there: it holds the number of the instrument that plays the note. They take plain numbers, edited like a parameter lane (click a cell or just type a digit, then Enter to save and move down):

| Column | What it holds |
| --- | --- |
| **Ins** | An instrument's number (shown before its name in the browser's **Project** folder), which plays that note and the notes below it in the column until another is set. |
| **Vol** | 0–100. |
| **Pan** | −50 (left) to +50 (right), or `C` for centre. |
| **Dly** | Starts the note that many percent of a row late, 0–99. |
| **Start** | Where a new note's sample starts and which way it plays: `50`, `<` or `50<`. See [Start, Pitch and Rtg](#start-pitch-and-rtg). |
| **Chance** | The percent chance, 0–100, that the note plays. Blank always plays. See [Chance](#chance). |
| **Pitch** | A glide or slide: `G8`, `U12` or `D3`. |
| **Rtg** | How many times the note plays in its row, 2–32, optionally with a change in level per hit, such as `4:-25`. |

Hidden sub-columns still play.

## Start, Pitch and Rtg

These sub-columns — the **Start**, **Chance**, **Pitch** and **Retrigger** checkboxes in **⋯** — sit to the right of **Dly** beside every note column, so each of a track's note columns has its own. Returns and the master hold no notes, so they have none. Hidden sub-columns still play, and hiding one never deletes what it holds.

**Start** configures a new note:

| Cell | What it does |
| --- | --- |
| `0`–`99` | Starts the sample that percent of the way in (the instrument's `begin`). |
| `<` | Plays it backwards (`rate` of −1). |
| `50<` | Both. |

**Rtg** is a number from 2 to 32: the note plays that many times within its row, like a stutter. The last hit is held as the note would be. After a `:` it takes how much each hit's level changes from the one before, from −100 to 100 percent: `4:-25` plays four hits, each 75% as loud as the last, and `4:-100` silences every hit after the first. A positive change builds up instead: in `4:50` each hit is 50% louder than the one before and the last plays at the note's own level, so no hit is louder than the note.

**Pitch** takes a letter and a whole number, **G**, **U** or **D**:

| Cell | What it does | Accepted |
| --- | --- | --- |
| **G** | Glides: instead of starting a new note, the note held in the column slides to this row's pitch over that many eighths of a row (**G8** is one row, **G16** two, **G4** half). The sound carries on unbroken, pitch moving evenly in semitones (equal frequency ratios over equal time), so it is a portamento on any instrument. With nothing held, or after another instrument's note, it is an ordinary note. | `G0`–`G99` |
| **U**, **D** | Slides the note held in the column up or down that many semitones during this row, at a constant musical rate. Repeat down the column for a longer slide. | `U1`–`U99`, `D1`–`D99` |

A glide keeps one voice and a retrigger restarts it, so a row cannot have both: entering one where the other is refused with a message, and a project that has both is rejected. A slide and a retrigger can share a row.

Type into them like any other cell: click it or just start typing, **Enter** to save and move down by **Step**, **Escape** to cancel, and clearing the cell removes the setting.

**Enter** on a cell you have not started typing in, or a double click, opens its **editor** instead: a small panel with named fields and units (Offset and Backwards for Start; Movement and Amount for Pitch; Hits and Level change for Rtg; Chance), a preview of the cell's text and what it means in words, and **Apply**, **Clear** and **Cancel**. **Enter** applies and **Escape** cancels. An entry that is out of range stays in the editor with a note saying what the field takes, and nothing changes until it is valid. Applying is one undo step. If a glide and a retrigger would share the row, the editor says so and offers **Replace the retrigger** (or the glide), which does both changes as one undo step. Hovering over a filled cell shows the same meaning in words. Lower case is tidied to upper case, so `g8` becomes `G8`; an entry that is not valid is refused, with a hint saying what the cell takes, and the cell keeps what it had.

Each acts on the note in its own row, or on the note held above it in the same column. An **OFF** or a **CUT** ends that hold, so a slide below one has nothing to move. **Start** needs a note in its own row to start something. Whatever a setting needs, the instrument has to have the parameter it sets: one it cannot use is ignored, and nothing else about the row changes. Samplers made by **♪** import or the scissors have `begin` and `rate`, so **Start** works on them.

Entering a **Start** or **Rtg** value plays the note it acts on at once, through the track's effects, so you can try a start point or a stutter without playing the pattern. Glides and slides are heard when the pattern plays.

### Chance

**Chance** is a whole percent from 0 to 100; blank is certain (100). Each time the pattern plays, every note with a chance rolls afresh, so loop passes and restarts differ. A note that does not play leaves the note before it sounding, as if the cell were empty, and drops its Start, Pitch and Rtg with it. A whole retrigger plays or not as one. An empty, **OFF** or **CUT** row ignores its Chance.

A row with only a Pitch or Rtg and no new note is never rolled. An export draws once for the whole song, and the mix and every stem share the same draws.

Cells are stored as the text you typed, so they copy, paste and save as `G8`; they move with their note when a block is inserted, deleted or pasted, and undo covers them.

## Sliced samples

The **✂** button above the instrument list makes a new instrument from the current sampler's sample, cut into 4, 8, 16 or 32 equal pieces, into any number from 2 to 48 (**Pieces**, then **Go**), or **At transients**, which finds where the hits of the sample begin (up to 48, at least 60 ms apart, each cut a millisecond before the hit so the attack is whole) and starts a piece at each.

Each key plays a piece, counting up from **C-4** (C#4 the second, and so on), at its recorded pitch; keys outside the pieces are silent. Write a pattern of keys to rearrange a break, and add **Rtg** and **Start** to the pieces.

### The Sample tab

The **Sample** tab of the bottom dock (View → Sample) draws the waveform of the current sampler. For a sliced instrument it marks each cut with the key that plays the piece after it. **Click** a piece to hear it. **Drag** a marker to move a cut (the code is rewritten when you let go, and applied), **double-click** the waveform to add a cut, **double-click** a marker (or select it and press Delete) to remove it. The first cut stays at the start and a sample keeps at least two pieces. **Find hits** moves the cuts to where the hits begin, and **Cut equally** starts again with that many equal pieces. A sampler that is not cut shows its waveform too; click to hear it. A kit has no single sample to show.

**Stretch ×** changes duration while preserving the selected pitch: `2` lasts twice as long, `0.5` lasts half as long, and `1` uses ordinary playback. Choose **Granular** for fixed grain positions and a lower CPU cost, or **WSOLA** to align neighbouring waveforms for smoother joins. **Transient WSOLA** protects detected forward attacks and stretches the bodies between them. Changing either control rewrites and applies the instrument code; the choice is saved with the instrument. The range is 0.25 to 4, and sliced instruments stretch each piece. A kit exposes `stretch` and `stretch_mode` through its instrument parameters and code.

**Estimate beats** uses detected hit spacing to suggest the number of beats in the whole sample and shows the corresponding recorded BPM. It fills the beat-count field without changing the instrument, cuts, loop points or existing fit. Review it by ear, use **÷2** or **×2** if it picked half/double time, then press **Fit to tempo**. You can also edit the count manually. The estimate works best on trimmed rhythmic loops, looks initially between 60 and 200 bpm, and allows subdivisions and missing hits. Fewer than four detected positions or weak rhythmic evidence leaves the current count alone and asks for a manual value. It cannot reliably determine the intended beat level from hit spacing alone.

**Fit to tempo** plays the sample in time with the song at any tempo, keeping its pitch. Say how many beats the whole sample lasts as recorded (**lasts 4 beats**) and press **Fit to tempo**: that sets the instrument's `from_bpm` to the tempo the sample was recorded at, and every note is stretched by that tempo over the song's tempo at the note, on top of its own **Stretch ×**. A four-beat loop that lasts two seconds is a 120 bpm loop, so it fits a 100 bpm song by lasting 1.2 times as long, and it follows tempo changes, pattern tempos and ramps from note to note (a note keeps the stretch it started with). **Off** sets `from_bpm` to 0, which leaves the sample alone. The stretch is held between 0.25 and 4. In a sliced instrument each piece is stretched the same way, so a rearranged break stays in time and each piece still starts on its hit; that is a better way to fit a break than stretching it whole. A fitted instrument is stretched when it is auditioned and when MIDI plays it too.

WSOLA can reduce beating on sustained sounds. Transient WSOLA protects up to 15 ms of detected attacks; quiet or closely spaced hits can still be missed, and reverse playback uses ordinary WSOLA. If `gate` is 1, a note can end before the stretched sample does: lengthen the note, or set `gate` to 0 to hear the whole sample.

For a whole sample, choose **Forward loop** or **Ping-pong loop**, then set **Start** and **End** as percentages, or drag their markers on the waveform. The shaded region repeats after the intro has played. This edits the existing sampler rather than making slices. Turning looping on enables `gate` so OFF or the next note can end it. **Loop off** keeps the points for later. Points are saved in the instrument code. Edits also reach held playback and MIDI notes, taking effect at their next loop wrap or ping-pong turn. Start must be before End; **Crossfade** sets the overlap in milliseconds of source audio (0 turns it off). It smooths wraps and ping-pong turns, including reverse and stretched playback, and is capped at half the usable loop span. The overlap shortens each repeat; longer fades can soften attacks.

The code is ordinary (`chops = amen.slices(16)`, or `amen.slices([0, 0.18, 0.43])` for pieces that start at those shares of the sample, with a small wrapper), so use `.beats(4)` and `.choke(...)` on the sample as the [language reference](/reference/language/samples/) describes. An instrument is sliced when it has a `sample` parameter and the code cuts one sample into pieces.

## MIDI note entry

Choose a keyboard under **MIDI**, then **Ch** (All or 1–16). **↻** refreshes inputs after connecting a device.

With record mode off, keys play the selected instrument on the cursor's track; previews follow note-off and support held chords and velocity. With it on, each key press writes its absolute pitch, instrument and hardware velocity into the current note column and advances by **Step**. A sub-column or parameter lane targets its track's note column; returns and the master use the last musical track. During playback, notes go to the nearest row without stepping. Note-off releases the preview; recorded note lengths follow the usual note/OFF rules. Pitches C-0 through B-8 are supported. Device and channel changes, unplugging, window blur and Stop release previews. MIDI is off until an input is selected, and the input selection is separate from the saved song.
