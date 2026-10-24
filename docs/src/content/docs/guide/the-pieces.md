---
title: The pieces
description: The server, the sequencer and the tracker — what each one does, and which parts of a file each one reads.
---

niminal is one language and three programs. You always run the server; you pick one of the two clients for the rest.

| Program | What it is | You run it when |
| --- | --- | --- |
| `niminal` | The audio server. Compiles instruments, effects and routing into sound, and plays the notes it is sent. | Always — it is what you hear. |
| `nimlive` | The live sequencer, and the language server behind the editor extension. Owns tempo, meter, clips, scenes, patterns and arrangements, and sends the server notes stamped with a time. | You want to arrange, loop or live-code, or to render a file to WAV. |
| **tracker** | A desktop app with a pattern grid, a mixer and a code pane. Keeps its own project of patterns, tracks, instruments and effects. | You would rather type notes into a grid than into text. |

The [tracker](/reference/tracker/) talks to the server directly and does not need `nimlive`. More than one client can play on the same server at once: a client's notes belong to its own connection.

## Two halves of one file

A `.nml` file is a single language, but its two halves are read by different programs.

- **The server's half** — instruments, effects, samples, envelopes, functions, routing, modules. `niminal` compiles these into sound. This is most of the [language reference](/reference/language/).
- **The sequencer's half** — tempo, meter, clips, scenes, mini-notation patterns, arrangements. `nimlive` reads these and turns them into notes with times; the server never sees that text.

That is why every page of the language reference says which program runs what it describes: **Compiled by niminal**, **Read by nimlive**, or **Read by both programs**. If you only want to write instruments and play notes from a program of your own, you can ignore the sequencer's half and use the [engine API](/reference/engine-api/) directly.

The tracker is the same language again for its instruments and effects, in its own project file rather than a `.nml` file.

## Which one do I need?

| I want to… | Use |
| --- | --- |
| hear an instrument I just wrote | `niminal server`, and any client to play it |
| loop a pattern and change it while it plays | nimlive — the [editor extension](/reference/editor/) is the easiest way |
| type notes into a grid, with a mixer and effect chains | the [tracker](/reference/tracker/) |
| turn a file into a WAV | [`nimlive render`](/guide/offline-rendering/) |
| play notes from my own program | the [engine API](/reference/engine-api/) |
| replay a session a server recorded | [`niminal replay`](/reference/server/cli/) |
