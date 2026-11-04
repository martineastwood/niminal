---
title: Score model
description: Events, sections, clips and scenes, and the .nms JSON format.
---

**Played by niminal.** The engine plays score events; a `.nms` file or a client of your own can be what writes them.

The engine can play **score events** that do not come from niminal source: a `.nms` file on the command line, or `events.send` / `clip.define` on a live server. The orchestra still defines the instruments; the score times and names the notes, clips and scenes.

## Elements

| Element | Contents |
| --- | --- |
| **Event** | Target (instrument or clip), start, duration, named arguments with units |
| **Section** | Named finite list of events with a length (`.nms` version 1) |
| **ClipDef** | Named clip: mini-notation lanes and/or a finite event list (wire API) |
| **SceneDef** | Named list of clips, started and stopped together |
| **Arrangement** | Scenes over time, in `.nml` |

Endless patterns (mini-notation, grids) stay in niminal and are evaluated by the server; they are not stored as finite score events until they play.

## Time

Positions and durations use the same unit strings as the language: `0beat`, `1/4beat`, `250ms`, `1bar`, and so on. Tempo and meter come from the `.nml` file you render against (or the running session).

## Wire format (`.nms`)

JSON. Values keep units as strings. Every file has a format **`version`**, which is 1: a single section.

### A single section

```json
{
  "version": 1,
  "section": "phi",
  "length": "4beats",
  "events": [
    { "at": "0beat", "dur": "1/2beat", "target": "saw_lead", "args": { "freq": "e5", "amp": "-12db" } },
    { "at": "1/2beat", "dur": "1/2beat", "target": "saw_lead", "args": { "freq": "g5", "amp": "-12db" } }
  ]
}
```

| Field | Meaning |
| --- | --- |
| `target` | Instrument or clip name in the `.nml` file / session |
| `at`, `dur` | Start and hold time, with units |
| `args` | Named parameters; strings with units or numbers, checked like orchestra code |

```sh
niminal render examples/saw_lead.nml --score examples/phi.nms --out phi.wav
```

Live: define the same clips with **`clip.define`** / **`scene.define`**, or send timed events with **`events.send`** ([Sequencer protocol](/reference/protocol/)). In `.nml`, `load("score/riff.nms")` turns a version-1 section (or a one-clip project) into a clip.
