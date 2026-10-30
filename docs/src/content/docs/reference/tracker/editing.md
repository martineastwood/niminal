---
title: Selecting and editing
description: Blocks, shortcuts and undo in the tracker.
---

## Selecting

Click and drag across notes, or use **Shift+arrows** / Shift+click, to select a rectangular block. Drag from inside a selection (in record mode) to move it, or Alt/Option-drag to copy; right-click for a menu of actions. **Cmd/Ctrl+A** selects all notes. Use normal copy and cut and paste shortcuts to exchange tab-separated note blocks. Paste clips to the pattern's bounds, and Delete clears the selection.

## Whole tracks and patterns

**Cmd/Ctrl+Shift+A** (or Edit → Select Track) selects every row and column of the track the cursor is in, and **Cmd/Ctrl+A** selects the whole pattern. Copy them and paste at the first row of another track or pattern (paste, like cut, needs record mode on) to move or repeat a track's notes, commands and lanes between patterns or onto another track; the pasted block is clipped to the pattern and one undo step brings the old cells back.

**Duplicate as variation** copies a pattern into the sequence, and **⧉ Track** copies a track throughout the song. **Edit → Cut Track** copies the whole track and clears it in one step. **Edit → Reverse Selection** turns the selected rows upside down in each selected column, and **Edit → Randomize Selection** gives every note or number in a column a random one between that column's lowest and highest in the selection (rests, OFF and CUT stay put, and a column that has only one value is left alone). Both are one undo step and need record mode.

## Jumping and selecting like a spreadsheet

**Cmd/Ctrl+↑ / ↓** jump within the column as Excel does: along a run of notes to its last one, from a gap to the next note, and to the first or last row when there is nothing more. **Cmd/Ctrl+← / →** go to the start or end of the track the cursor is in, then on to the next track's. **Cmd/Ctrl+Home** goes to the first cell and **Cmd/Ctrl+End** to the last. Add **Shift** to any of them to select as you go, so **Cmd/Ctrl+Shift+↓** selects from the cursor to the end of a run, or to the bottom of the pattern. **Ctrl+Space** selects the whole column and **Cmd/Ctrl+Shift+A** the whole track.

## Block editing

With record mode on, these move or fill whole blocks. Every one is a single undo step, and columns drawn by a graphical envelope are left alone. Where Renoise has the same command the key is the same; the others use Cmd on macOS and Ctrl elsewhere.

| Key | What it does |
| --- | --- |
| **Delete** | Clears the selected cells. |
| **Backspace** | Deletes the row at the top of the selection and pulls the rest of the track up. |
| **Insert** (or **Cmd/Ctrl+Enter**) | Inserts a blank row there and pushes the rest down. |
| **Cmd/Ctrl+Shift+Insert**, **Cmd/Ctrl+Shift+Delete** | The same two, for just the selected columns. |

Inserting and deleting move notes, sub-columns and lanes together. What moves off the end of the pattern is lost, and undo brings it back.

| Key | What it does |
| --- | --- |
| **Cmd/Ctrl+Option/Alt+↑ / ↓** | Transposes the selected notes by a semitone. |
| **Cmd/Ctrl+Option/Alt+Shift+↑ / ↓** | Transposes by an octave. |
| **Cmd/Ctrl+I** | Interpolates: select three or more rows and each volume, pan, delay or parameter column is filled with a straight line between its first and last selected values. |
| **Cmd/Ctrl+D** | Copies the selection onto the rows just below it and selects the copy, so pressing it again carries on down the pattern. |
| **Page Up / Page Down** | Moves by a bar of the pattern's meter. |
| **Home / End** | Goes to the first or last row. |
| **F9 / F10 / F11 / F12** | Jumps to the start and the 25%, 50% and 75% points. |

Shift extends the selection as it does with the arrow keys. Option+↑ / ↓ alone chooses the instrument, as in Renoise, and Alt+Shift+↑ / ↓ move through the sequence. OFF, CUT, and notes that would leave C-0 to B-8 stay as they are.

## Sequence shortcuts

From the grid (the ones that change the sequence do nothing while it plays): **Alt+Shift+↑ / ↓** select the previous or next sequence step, **Alt+Shift+Home / End** the first or last, **Cmd/Ctrl+Insert** inserts a new blank pattern after it and **Cmd/Ctrl+Delete** removes the step.

## Undo

**Cmd/Ctrl+Z** undoes and **Cmd/Ctrl+Shift+Z** or **Cmd/Ctrl+Y** redoes project edits in chronological order, wherever focus is. Toolbar and envelope undo buttons use the same history. This includes instrument and effect source, parameter values, mixer settings, effect chains, envelopes, cells, timing, and sequence and track changes.

The last 100 edits are kept in memory; opening another project starts a fresh history. Source restoration requires **Apply** to change the compiled sound. Undoing sequence structure, pattern length or track count while playing stops playback first.
