---
title: The pieces
description: The server and the three ways to work with it — what each one does, and which parts of a file each one reads.
---

niminal is one language, one audio server and three ways to work with it. You always run the server (`niminal server`); then you open the client you like. All of them connect to it on port 7401.

| Program | What it is | You use it when |
| --- | --- | --- |
| `niminal` | The audio server. Compiles instruments, effects and routing into sound, and plays the notes it is sent. | Always — it is what you hear. |
| [**VS Code extension**](/reference/editor/) | Your editor, with highlighting, diagnostics, and evaluate-while-playing. It runs the live sequencer for you. | You write `.nml` files in VS Code or Cursor. |
| [**Stage**](/reference/stage/) | A desktop app for live coding: an editor beside scenes, lanes and meters, with the live sequencer built in. | You want to perform from one window. |
| [**Tracker**](/reference/tracker/) | A desktop app with a pattern grid, a mixer and a code pane. Keeps its own project of patterns, tracks, instruments and effects. | You would rather type notes into a grid than into text. |
| `niminal-live` | The live sequencer, on the command line, and the language server behind the editors. Owns tempo, meter, clips, scenes, patterns and arrangements, and renders a file to WAV. | You work from a terminal, or want to render. |

The tracker does not use the sequencer at all. More than one client can play on the same server at once: a client's notes belong to its own connection.

## Two halves of one file

A `.nml` file is a single language, but its two halves are read by different programs.

- **The server's half** — instruments, effects, samples, envelopes, functions, routing, modules. `niminal` compiles these into sound. This is most of the [language reference](/reference/language/).
- **The sequencer's half** — tempo, meter, clips, scenes, mini-notation patterns, arrangements. `niminal-live` reads these and turns them into notes with times; the server never sees that text.

That is why every page of the language reference says which program runs what it describes: **Compiled by niminal**, **Read by niminal-live**, or **Read by both programs**. If you only want to write instruments and play notes from a program of your own, you can ignore the sequencer's half and use the [engine API](/reference/engine-api/) directly.

The tracker is the same language again for its instruments and effects, in its own project file rather than a `.nml` file.

## Which one do I need?

| I want to… | Use |
| --- | --- |
| hear an instrument I just wrote | `niminal server`, and any client to play it |
| loop a pattern and change it while it plays | the [editor extension](/reference/editor/), or [Stage](/reference/stage/) |
| type notes into a grid, with a mixer and effect chains | the [tracker](/reference/tracker/) |
| turn a file into a WAV | [`niminal-live render`](/guide/offline-rendering/) |
| play notes from my own program | the [engine API](/reference/engine-api/) |
| replay a session a server recorded | [`niminal replay`](/reference/server/cli/) |
