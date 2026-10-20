---
title: Score model
description: Events, sections, projects, and the .nms JSON format.
---

The engine can play **score events** that do not come from niminal source: a `.nms` file on the command line, or `events.send` / `clip.define` on a live server. The orchestra still defines the instruments; the score times and names the notes, clips and scenes.

## Elements

| Element | Contents |
| --- | --- |
| **Event** | Target (instrument or track), start, duration, named arguments with units |
| **Section** | Named finite list of events with a length (`.nms` version 1) |
| **ClipDef** | Named clip: mini-notation lanes and/or a finite event list (`.nms` version 2 / wire API) |
| **SceneDef** | Named set of clips, one per track |
| **Project** | Clips, scenes and an optional arrangement (`.nms` version 2) |
| **Arrangement** | Scenes over time — in `.nml` or in a v2 project |

Endless patterns (mini-notation, grids) stay in niminal and are evaluated by the server; they are not stored as finite score events until they play.

## Time

Positions and durations use the same unit strings as the language: `0beat`, `1/4beat`, `250ms`, `1bar`, and so on. Tempo and meter come from the `.nml` file you render against (or the running session).

## Wire format (`.nms`)

JSON. Values keep units as strings. Every file has a format **`version`**.

### Version 1 — a single section

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
| `target` | Instrument or track name in the `.nml` file / session |
| `at`, `dur` | Start and hold time, with units |
| `args` | Named parameters; strings with units or numbers, checked like orchestra code |

```sh
nimlive render examples/saw_lead.nml --score examples/phi.nms --out phi.wav
```

### Version 2 — a project

Clips, scenes and an optional arrangement. The CLI compiles them to `.nml` and renders the project's arrangement (or the one named on the command line).

```json
{
  "version": 2,
  "clips": [
    { "name": "theme", "lanes": { "notes": "c4 e4 g4 b4" } }
  ],
  "scenes": [
    { "name": "verse", "entries": { "lead": "theme" } }
  ],
  "arrangement": {
    "name": "song",
    "sections": [{ "scene": "verse", "length": "2bars" }]
  }
}
```

```sh
nimlive render examples/live_controls.nml --score examples/project.nms --out project.wav
```

Live: define the same clips with **`clip.define`** / **`scene.define`**, or send timed events with **`events.send`** ([Sequencer protocol](/reference/protocol/)). In `.nml`, `load("score/riff.nms")` turns a version-1 section (or a one-clip project) into a clip.
