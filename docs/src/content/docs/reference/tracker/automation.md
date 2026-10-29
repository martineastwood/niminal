---
title: Track commands, automation and envelopes
description: Setting levels and effect parameters from the grid, and drawing automation, in the tracker.
---

## Track commands

A track's **Fx** columns, the command columns beside its notes (each track starts with one; **⋯** adds up to four), also take commands for the track itself, written like a note's [commands](/reference/tracker/grid/#commands): a letter and two hex digits, or four hex digits for an effect. A return or the master has no notes, so its command columns (**Cmd**, one to four, set in its **⋯** menu) take only these.

| Command | What it does |
| --- | --- |
| `Lxx` | Sets the track's volume: `00` is silent, `FF` is +3 dB (about `B4` is 0 dB). |
| `Pxx` | Sets its pan: `00` is left, `80` centre, `FF` right. |
| `xyzz` | Sets parameter `y` of the effect in place `x` of the track's chain to `zz`, from its least at `00` to its most at `FF`. Both counts start at 1, in the order of the code pane's FX view and of the effect's sliders, so `12FF` turns the first effect's second parameter right up. The parameter is one hex digit, so a command reaches an effect's first 15 parameters; sliders and automation lanes reach all of them. |
| `0yzz` | Sets macro `y` of the track (from 1, up to 8) to `zz`, from 0% at `00` to 100% at `FF`. It moves every parameter linked to that macro. |

Hovering a cell, or typing one, explains it in words (`L80 sets the track volume to -1.1 dB`, `12FF sets parameter 2 of effect 1 (Echo) …`, `0180 sets macro 1 (Open) to 50%`).

Type into a cell as for a note's command: click it or start typing, **Enter** to save and move down by **Step**. The setting holds from that row until another command changes it, across pattern boundaries. A command that names an effect, parameter or macro the track does not have is skipped, and so is one for an effect that is bypassed or whose parameter a macro drives.

Effect commands count effects by place, so the tracker keeps them on the effects they were written for: moving an effect in the chain renumbers its commands (and those of the effect it swapped with), and removing an effect clears its commands and moves the later effects' commands up a place; the status line says how many were cleared, and undo brings them back. A command for a place with no effect yet starts working when one is added there. Parameters are numbered in the order the effect's code declares them, and the tracker cannot follow a code edit, so add a new parameter at the end rather than in the middle, or the numbers after it shift. Track commands do not change the notes that are sounding; for that, automate an instrument's own parameters with an envelope.

## Automation lanes

Drawn automation goes in the **Automation** tab of the bottom dock, not in the grid. Each track, return and the master has up to four **lanes** in each pattern. Choose a track (select it in the grid), then the lane and its parameter in the tab's toolbar: **Vol** or **Pan** (under *Track*), or a parameter listed under the effect or instrument it belongs to, such as *Stereo delay* › **feedback**. **+** adds a lane, **−** removes the chosen one. A lane with an envelope must be cleared before it is retargeted or removed, and each parameter can have one lane per track. Connect and Apply effects to expose their parameters. A lane can also target one of the track's macros, listed under **Macros**, so one envelope moves every parameter linked to it. Tracker knobs use the same macro evaluation as the language: frequency sweeps are geometric, decibel sweeps are even in dB, and other values move linearly. Reversing a link reverses its sweep.

Values are read and shown with their unit: a level as `-6 dB` (range −60 to +6 dB), pan as `-100%` left to `100%` right, frequency as `2400 Hz` or `2.4 kHz`, time as `250 ms` or `0.25 sec` (a bare number is milliseconds), a parameter declared `0..1` as `35%`. Anything else is its physical value and optional unit, as shown on the slider.

## Writing a slider into a lane

Connect and **Apply** an effect to expose its scalar parameters. For example:

```niminal
fx soften(cutoff: hz = 2400hz, resonance: 0..1 = 0.2) {
  out = in.lpf(cutoff: cutoff, res: resonance)
}
```

Move a slider or edit its numeric value for live control. Built-in and custom effects use the same generated panel. **↳**, or right-clicking a slider, records the exact value as an envelope point at the cursor's row on that effect's track, and switches the envelope on: it reuses the lane for that parameter, takes an empty unbound lane, or adds a new lane if space permits. A point already on that row is replaced.

Effect automation stays attached to the device it was written for, so reordering a chain keeps lanes on the same device. Changing code or loading a preset preserves manual values and lanes. Renamed or removed parameters are labelled missing and skipped until repaired or restored.

## Instrument parameters

Editing an instrument shows its parameters as sliders beside the code (after **Apply**), for any parameter except `freq`, `gain` and `pan`. Notes start with those values.

**↳** records a value as an envelope point on the selected track for that instrument: the Automation tab lists the parameters of the instruments that track plays, and any a lane already points at. From that row on, notes of that instrument start with the envelope's value instead.

An envelope also changes the notes that are sounding: it sets the parameter on every note of that instrument still sounding on the track, and sweeps it across each row, stepping or ramping as its mode says. So a held note can be filtered open, faded, or given vibrato by automating the instrument's own parameters — a `depth` or `rate` that the code feeds to an LFO, say. A note that has been released is not changed.

The **−** beside a slider hides it (**n hidden · show all** brings them back); a hidden parameter keeps its value and its lanes.

## LFOs

Use **∿ LFO** beside a parameter to add or remove modulation. Choose sine, triangle, saw or square, the lines per cycle, and a normalized minimum and maximum.

The LFOs run in the engine, so they keep going while you audition with playback stopped; **▶** restarts them in phase with the song. A row value targeting an LFO-controlled parameter disconnects that mapping and takes control until Play resets it or the LFO is toggled off and on. Other parameters keep modulating.

### LFOs on instrument parameters

The **∿** beside an instrument slider gives that parameter an LFO of its own, per note. It swings the parameter around the slider's value: pick the shape, the rate in Hz and the depth as a percent of the parameter's range (100% sweeps all of it). It works by writing two lines at the top of the instrument's code, a `// mod` comment that records the settings and the line it describes:

```nml
// mod cutoff: tri 2hz 50%
cutoff = (cutoff + lfo(2hz, shape: tri) * 1900hz).clamp(200hz..4khz)
```

The code is applied at once, so you hear it, and every note runs its own LFO from the start of the note. You can edit or delete the lines by hand; the **∿** reads the comment back. Parameters in decibels can't be modulated this way, and the rate is in Hz rather than rows, so it doesn't follow tempo changes.

## Envelopes

The **Automation** tab shows the envelope of the chosen lane. **Use envelope** switches it on or off; a lane's points are kept either way, and recording a slider switches it on.

- **Points**: click to add a point, drag a point to move it, right-click to delete it.
- **Draw**: click and drag to paint a sequence of points.
- **Snap**: row, beat, or 1/16 row. The horizontal axis is pattern time; the vertical axis uses the target parameter's range.
- **Linear** ramps continuously between points; **Hold** changes only at points. Volume and dB controls interpolate in linear gain, which the graph displays as a curve on its dB axis.
- Select a point and use **Row** / **Value** for exact placement and unit-aware values. **Delete point** removes it.
- The undo and redo buttons use the shared project history, and **Cmd/Ctrl+Z** works with the graph focused. One drag or draw gesture is one undo entry. **Clear** removes the points and switches the envelope off.

Envelopes belong to a pattern and repeat with it. Reused pattern references share the same envelope; duplicating a variation makes an independent copy. Pattern shortening retains hidden points. Before its first point, a parameter retains its current value; after the last, it holds that value until another event changes it. Envelopes take control from an LFO at their first active segment.

Editing during playback affects newly scheduled rows; rows already queued keep their earlier shape, which at slow tempos can reach one row beyond the lookahead.

## Starting and stopping

Starting or stopping restores the saved manual volume, pan and parameter values rather than leaving the last pattern value latched as the next play's starting point, and LFO mappings continue to own their targets. Source edits take effect on **Apply**; live parameter and LFO edits apply at once.
