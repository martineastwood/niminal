---
title: Mixer, returns and sends
description: Levels, scopes, groups and parallel effects in the tracker.
---

## The mixer

**Mixer** in the toolbar (or the bar above the grid) replaces the grid with a strip for every track, return and the master: a scope of the channel's finished signal (after its effects), pan, a fader with a level meter (double-click resets it to 0 dB, arrow keys nudge it), **M** mute on tracks and returns, **S** solo on tracks, and **FX** on every strip. A thin scope of the master runs across the top of the window. Scopes need the engine connected, and update about twenty times a second.

While any track is soloed, only soloed tracks feed the mix and the returns.

## Returns and outputs

**+ Return** in the **Mixer** adds a return track (up to eight; a strip appears only once its return exists) with its own level, pan, mute and effect chain, feeding the master. A track's **output** (the dropdown on its strip) is *Master*, one of the returns, or *None*.

Returns have an output and send devices of their own, but only to *later* returns (A can feed B, never the reverse), so signal can't loop. Nested groups work the same way.

Point several tracks at one return to make a group: kick, snare and hats into a Drums return with a compressor on it, heard only through that compressor.

## Sends

To send some of a track in parallel instead of routing all of it, add a **Send** device from the track's FX view (**+ Add effect → Send**) and choose its return on the device.

By default a send is post-fader: it takes the track's finished signal after its effects, level and pan, so it follows the fader, and a muted track sends nothing. Tick **Pre-fader** on the device to take the signal at the device's own place in the chain instead: the fader no longer affects it, effects before it shape what is sent, and effects after it do not. Either kind of send can **Mute source** so the track is heard only through its sends: on a pre-fader send it silences everything after it in the chain, and on a post-fader send it silences the track's own output while the send still follows the fader.

A send's amount is an ordinary parameter, so it has lanes, envelopes and an LFO like any other. Put the effect on the return, with its mix fully wet. A track with output *None* and a send is also heard only through the send.

**×** removes a return: outputs pointing to it go back to the master. Send destinations are stored by return position, so a send aimed at the removed position feeds the next return if one takes its place, or becomes silent if that position no longer exists, while sends aimed at later returns follow them as they shift down. Check send destinations after removing a return.

## Returns and the master in the grid

After the tracks, the grid draws a narrow column for each return and one for the master (white), with track commands only: no notes, no sub-columns. Set a return's or the master's level and pan with `Lxx` and `Pxx`, and an effect's parameter with `xyzz` (see [Track commands](/reference/tracker/automation/#track-commands)); draw anything else in the **Automation** tab, with **Vol**, **Pan** or one of the strip's own effects' parameters (no instruments). A return column also has **M**; the master has no mute.

Selecting one of these columns makes it the strip the dock's FX tab, the **↳** buttons and the Automation tab work on, and **Tab** walks through them after the last track. Every pattern has its own commands and lanes for each, copied into a variation.
