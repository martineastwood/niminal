---
title: Score model
description: Events, lanes, sections, time, and the JSON wire format.
---

The engine understands one **score model** as data. niminal notation, Python, a tracker, or MIDI all produce it — separate from the orchestra, so any tool can write music for the same instruments.

## Elements

| Element | Contents |
| --- | --- |
| **Event** | Target (instrument or track), start, duration, named arguments with units, optional id |
| **Lane** | Curve over time for one parameter, or a pattern |
| **Section** | Named finite container of events and lanes; reusable and launchable |
| **Arrangement** | Sections over time plus song-level lanes (tempo, meter, automation) |

## Time

Musical time (beats/bars via tempo and meter map) or absolute seconds — both in one score.

- Positions: `bar 5 beat 3`; offsets: `1/8beat`, `250ms`.
- Tempo and meter are **lanes** on the arrangement.

```nml
arrangement song {
  sections: [dawn.over(16 bars) build.over(32 bars) theme.over(24 bars)]
  tempo:    env[84bpm (64 bars) 84bpm (8 bars).log 96bpm]
  meter:    [4/4 4/4 7/8 4/4].per_section
}
```

## Validation

Every event is checked against its target’s signature with the same type and unit checker as orchestra code.

## Updating running notes

Events can carry an **id** so later updates reach the running voice:

```nml
n = pad(freq: c3) for 8beats
at beat 4  n.set(freq: d3)
```

Ids are scoped per section launch; updates to ended voices are ignored with a warning.

## Finite vs endless material

- The score model holds **finite** material (known lengths).
- **Endless patterns** are niminal values the daemon evaluates, emitting events over time.
- External clients can stream timestamped events slightly ahead of the clock.

## Wire format

JSON over the protocol (MessagePack later for huge scores). Saved as **`.nms`** files. Values keep units as strings. Format version on every file — no separate text score; human notations belong to clients.

```json
{
  "section": "phi", "length": "16bars",
  "events": [
    { "at": "0beat", "dur": "1/4beat", "track": "lead", "args": { "freq": "e4", "gain": "-6db" } }
  ],
  "lanes": [
    { "target": "bass.tone",
      "points": [ { "at": "0beat", "value": "300hz" }, { "at": "4beats", "value": "1.2khz", "curve": "exp" } ] }
  ]
}
```

Example clients (Python, tracker `.nmt`, line `.line`, MIDI import) ship with the project; each owns its notation and produces sections for `load(...)` or the protocol.
