---
title: Parameter lanes, LFOs and envelopes
description: Automating levels, effect parameters and instrument parameters in the tracker.
---

## Parameter lanes

Each track starts with a **Track volume** lane. A level in dB is shown in the cell as a bare number (`-6`, `-47.5`, to one decimal place at most); hover a cell for its unit.

Press **P** with the cursor on a parameter lane to choose its parameter from the keyboard (a list with a search box: type part of a name, **Enter** takes the first match, **Escape** goes back to the grid), or use the lane's header dropdown, to choose **Vol** or **Pan** (under *Track*), or a parameter listed under the effect or instrument it belongs to, such as *Stereo delay* › **feedback**. Connect and Apply effects to expose their parameters.

**⋯** in the track header adds or hides up to four lanes per track per pattern. A lane with values must be cleared before retargeting or removing it, and each parameter can have one lane per track.

Click a cell and type a readable value, then **Enter** to save and advance by **Step**. **Escape** cancels. Clicking away saves without advancing. Arrows navigate cells; Enter or a number starts keyboard editing. Blank cells hold the previous value, including across pattern boundaries, and playback starts from saved manual settings. Delete clears selected values.

| Parameter | What you type |
| --- | --- |
| Volume | `-6 dB` (range −60 to +6 dB) |
| Pan | `-100%` left, `0%` centre, `100%` right |
| Frequency | `2400 Hz` or `2.4 kHz` |
| Time | `250 ms` or `0.25 sec`; bare numbers mean milliseconds |
| A parameter declared `0..1` | `35%`; bare numbers mean percent |
| Anything else | Its physical value and optional unit, as shown on the slider |

Populated cells have a **⋮** handle: drag vertically to adjust; hold Shift for finer movement. One drag is one undoable edit. Click and drag elsewhere selects rectangles as for notes. Copy and paste use readable values with units and reject values outside the destination lane's range; no hex commands or 8-bit value quantization are used.

## Writing a slider into a lane

Connect and **Apply** an effect to expose its scalar parameters. For example:

```niminal
fx soften(cutoff: hz = 2400hz, resonance: 0..1 = 0.2) {
  out = in.lpf(cutoff: cutoff, res: resonance)
}
```

Move a slider or edit its numeric value for live control. Built-in and custom effects use the same generated panel. **↳**, or right-clicking a slider, writes the exact value into a matching lane on that effect's track at the current row: it reuses a matching lane, assigns an empty unbound lane, or adds a new lane if space permits.

Effect automation stays attached to the device it was written for, so reordering a chain keeps lanes on the same device. Changing code or loading a preset preserves manual values and lanes. Renamed or removed parameters are labelled missing and skipped until repaired or restored.

## Instrument parameters

Editing an instrument shows its parameters as sliders beside the code (after **Apply**), for any parameter except `freq`, `gain` and `pan`. Notes start with those values.

**↳** records a value into a lane on the selected track for that instrument: the lane picker lists the parameters of the instruments that track plays, and any a lane already points at, and a lane for an instrument no note on the track uses is dimmed, because it has no effect. From that row on, notes of that instrument start with the lane's value, or its envelope's value, instead.

A lane also changes the notes that are sounding: a value on a row sets the parameter on every note of that instrument still sounding on the track, and an envelope sweeps it across the row, stepping or ramping as its mode says. So a held note can be filtered open, faded, or given vibrato by automating the instrument's own parameters — a `depth` or `rate` that the code feeds to an LFO, say. A note that has been released is not changed.

The **−** beside a slider hides it (**n hidden · show all** brings them back); a hidden parameter keeps its value and its lanes.

## LFOs

Use **∿ LFO** beside a parameter to add or remove modulation. Choose sine, triangle, saw or square, the lines per cycle, and a normalized minimum and maximum.

The LFOs run in the engine, so they keep going while you audition with playback stopped; **▶** restarts them in phase with the song. A row value targeting an LFO-controlled parameter disconnects that mapping and takes control until Play resets it or the LFO is toggled off and on. Other parameters keep modulating.

## Envelopes

Click **⌁** beside a lane header to open the envelope editor in the dock's **Automation** tab. Choose a track and one of its named parameter lanes. **Use envelope** switches that lane between its saved row values and the envelope; both are retained in the project. The active envelope is shown in blue in the pattern grid, where clicking those cells opens the envelope editor. A slider's **↳** switches the matching lane back to row values.

- **Points**: click to add a point, drag a point to move it, right-click to delete it.
- **Draw**: click and drag to paint a sequence of points.
- **Snap**: row, beat, or 1/16 row. The horizontal axis is pattern time; the vertical axis uses the target parameter's range.
- **Linear** ramps continuously between points; **Hold** changes only at points. Volume and dB controls interpolate in linear gain, which the graph displays as a curve on its dB axis.
- Select a point and use **Row** / **Value** for exact placement and unit-aware values. **Delete point** removes it.
- The undo and redo buttons use the shared project history, and **Cmd/Ctrl+Z** works with the graph focused. One drag or draw gesture is one undo entry. **Clear envelope** removes the points and returns to row values.

Envelopes belong to a pattern and repeat with it. Reused pattern references share the same envelope; duplicating a variation makes an independent copy. Pattern shortening retains hidden points. Before its first point, a parameter retains its current value; after the last, it holds that value until another event changes it. Envelopes take control from an LFO at their first active segment, and envelopes and row values never control one lane at the same time.

Editing during playback affects newly scheduled rows; rows already queued keep their earlier shape, which at slow tempos can reach one row beyond the lookahead.

## Starting and stopping

Starting or stopping restores the saved manual volume, pan and parameter values rather than leaving the last pattern value latched as the next play's starting point, and LFO mappings continue to own their targets. Source edits take effect on **Apply**; live parameter and LFO edits apply at once.
