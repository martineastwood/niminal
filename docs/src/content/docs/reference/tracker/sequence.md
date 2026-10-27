---
title: Patterns, the sequence and timing
description: Song order, patterns, loops, sections, per-track cells, tracks and tempo in the tracker.
---

The left-hand sequence controls song order. Each step occupies one compact row; the **◂ / ▸** button hides or shows pattern names. Hover over the small action buttons for their labels.

## The sequence

- **+ Blank** creates an empty pattern and inserts it after the selected sequence step.
- **Reuse pattern** inserts another reference to the current pattern. Edits affect every entry referencing it; the pattern toolbar shows the reference count.
- **Duplicate as variation** creates an independent copy and inserts it after the selected step.
- Use the pattern selector on a sequence entry to change what it plays. **↑ Move / ↓ Move** reorder entries; **Remove step** removes a reference while keeping the pattern in the pattern pool. At least one sequence entry remains.

## Patterns

The pattern picker above the grid can open any pattern, including one not currently sequenced. Patterns have a name and **1–256 active rows**. Shortening a pattern keeps hidden notes, so lengthening it restores them.

## Looping part of the song

**⟲ Loop** repeats part of the sequence during **▶ Song**. Press it to loop the selected step, shift-click another step's number to loop everything between them, and press it again to clear the loop. The looped steps are outlined.

Song playback runs from where it starts up to the end of the loop, repeats the loop until you stop it, and ignores a loop that lies before its start. The loop can be set, moved or cleared while playing: playback carries on to the end of the pattern now sounding and loops from there. Steps added inside the loop join it, and the loop follows the sequence as steps are inserted or removed.

## Sections

**§ Section** starts a named section at the selected step, running up to the next section's start; press it on a step that starts one to remove it.

A section header sits above its steps: **▾ / ▸** collapses or expands the steps (the count shows how many are hidden, and a collapsed section lights up while one of its steps plays), the name is editable, and **⟲** loops exactly that section (press it again to clear). Sections only organise the list and don't change playback. Steps added after a section's first step join it, steps removed move later sections up, and a section left with no steps disappears. Sections are saved with the project.

## Track cells

Each sequence step has a small cell for every track at the end of its row (the sequence bar widens to fit them), so each step can differ per track without copying a whole pattern. This is the pattern matrix.

A cell is blank when the track plays the step's own pattern. **Click** a cell for a menu to mute that track in that step (**✕**) or have it play another pattern's notes and lanes (shown as that pattern's number, like **03**), or choose *Own pattern* to undo; **Shift-click** mutes or unmutes at once.

A redirected track plays the other pattern's rows from its first row, and falls silent after them if that pattern is shorter than the step. A muted track ends a note still sounding from the step before. The pattern editor marks a track that is muted or redirected in the selected step (the header is dimmed or underlined) because editing its cells there isn't heard.

A step's cells move with it when steps are inserted, moved or removed, and with a track when it is deleted and restored. Cell changes can be made while the song plays. Pattern-only playback (**▶ Pattern**) ignores them.

## Playing

- **▶ Pattern** repeats the currently edited pattern.
- **▶ Song** starts at the selected sequence entry and stops after the final entry, allowing releases and effect tails to finish.
- **■** cancels queued events and silences the connection.

Playback schedules rows 200 ms ahead of the sound. A long enough stall — a slow compile, a stalled window — skips the missed rows rather than playing them as a burst, and the window reports skipped rows and late events.

## Recording while it plays

With **● Rec** on and the pattern playing, the cursor rides the play position (up/down, Page and Home/End keys are locked; left/right still choose the column). A note key enters the note on the *nearest* row to the moment you press it, without stepping, so you can play along and overdub as the pattern loops. The **Vel** box applies as usual.

## Follow playback

**Follow playback** moves the editor to the sounding pattern at each sequence boundary. Selecting a pattern manually turns follow off so you can edit another section while listening. The sounding sequence entry remains highlighted.

## Tracks

Tracks have compact, colour-coded headers. Click a track name to select it, **double-click its name to rename** (Enter or click away saves; Escape cancels), **M** to mute, **S** to solo (a dot after the name means the track has effects; the pane's **FX** button opens its chain), and **⋯** for its note columns, sub-columns and parameter lanes — see [the grid](/reference/tracker/grid/).

**+ Track** adds a song-wide track while stopped, up to 16; scroll horizontally to reach tracks outside the viewport, and the row-number gutter stays visible. **⧉ Track** adds a copy of the selected track right after it while stopped, with its volume, pan, output, effects, and its notes, lanes and matrix cells in every pattern. **− Track** deletes the selected track across all patterns while stopped; **↺** restores the last deleted track, including its notes and mixer settings. At least one track remains. Track deletion and restoration are included in project undo.

## Timing

**BPM** and **LPB** set song defaults and can change during playback. **Pattern BPM**, pattern **LPB**, and **Meter** override timing for the selected pattern; **Use song timing** clears those overrides, including the pattern's swing.

BPM counts quarter notes per minute and LPB counts rows per quarter note. Meter groups rows: 7/8 with LPB 4 has 14 rows per bar, so set **Rows** to 14 or a multiple for complete bars.

**Tempo points** takes ascending `row:BPM` pairs, e.g. `0:125, 32:80` for a ritardando. BPM ramps linearly in row space between points, holds the last value, uses Pattern BPM before the first point, and steps to the first point rather than ramping into it. Points can reach the pattern end. Each pattern starts its own timing, including when reused or looped; a variation copies it independently. Playback, note holds, delays, envelopes, LFO row periods and WAV export all follow the timing.

**Swing** (0–75, in the toolbar for the song and in the pattern controls for a pattern) delays every second row of each pair of rows. For each pair, the shorter row's time multiplied by the swing percentage moves from the second row to the first, so the pair lasts as long as before, including during a tempo ramp: with straight rows, 50 makes the first row three times as long as the second. It is a swing amount, not a position within the pair. Swing acts on rows, so LPB sets the subdivision that swings: LPB 4 swings sixteenth notes. Pairs begin at each pattern's first row, and an odd last row is left alone. A pattern's own swing, even 0, replaces the song's; leave it blank to follow the song. Notes, delays, retriggers, glides, envelopes, the playhead and export all follow the swung rows, and a delay stays a fraction of its row, so it is still there when swing is turned off. A matrix alias plays with the swing of the step it is in.

Live timing edits affect rows beyond the scheduler's existing lookahead; queued events and sounding notes retain the timing they were given. Lengths written in beats in instrument or effect code are compiled at a fixed 120 BPM, so they do not follow pattern timing — use `sec` for anything meant to track it.

## What you can change while it plays

Effects, volume and mute are song-wide track settings, and instruments belong to the project, not to tracks. Patterns contain notes and parameter values.

Stop playback before changing sequence structure, pattern lengths, or track count. Notes, instrument assignments, parameter values, envelopes, track cells, loop ranges and section organisation can be edited while playing. Mixer controls, effect chains, returns and sends are live too: add, reorder, bypass or remove devices, change outputs or send destinations, and add or remove returns without stopping. Apply instrument or effect code while playing to hear the compiled changes.
