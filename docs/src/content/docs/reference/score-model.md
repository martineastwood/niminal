---
title: Score model
description: Events, sections, and the .nms JSON format.
---

The engine can play **score events** that do not come from niminal source: a `.nms` file on the command line, or `event.stream` on a live daemon. The orchestra still defines the instruments; the score only times and names the notes.

## Elements

| Element | Contents |
| --- | --- |
| **Event** | Target (instrument or track), start, duration, named arguments with units |
| **Section** | Named finite list of events with a length (a `.nms` file is one section) |
| **Arrangement** | Scenes over time in niminal source (see [Performance](/reference/language/performance/)) — not part of the `.nms` file format |

Endless patterns (mini-notation, grids) stay in niminal and are evaluated by the daemon; they are not stored as finite score events until they play.

## Time

Positions and durations use the same unit strings as the language: `0beat`, `1/4beat`, `250ms`, `1bar`, and so on. Tempo and meter come from the `.nml` file you render against (or the running session).

## Wire format (`.nms`)

JSON. Values keep units as strings. Every file has a format **`version`**.

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

Example in the repo: `examples/phi.nms`, played with:

```sh
niminal render examples/saw_lead.nml --score examples/phi.nms --out phi.wav
```

Live: send the same events with **`event.stream`** ([Daemon protocol](/reference/protocol/)).

Arrangements, clip lanes, and tempo maps are written in `.nml`, not in `.nms` today.
