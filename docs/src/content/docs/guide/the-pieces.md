---
title: The pieces
description: The server and the three ways to work with it — what each one does, and which parts of a file each one reads.
---

niminal is one language, one audio engine and two ways to work with it: Stage and the VS Code extension. The engine lives inside Stage, which plays through your sound card. The extension plays through Stage. For testing and for building your own clients, `niminal server` runs the same engine as a command and serves it on port 7401.

| Program | What it is | You use it when |
| --- | --- | --- |
| [**Stage**](/reference/stage/) | A desktop app for live coding: an editor beside scenes, lanes and meters, with the audio engine and the live sequencer built in. | You want to perform from one window. |
| [**VS Code extension**](/reference/editor/) | Your editor, with highlighting, diagnostics, and evaluate-while-playing through Stage. | You write `.nml` files in VS Code or Cursor. |
| [`niminal`](/reference/server/cli/) | The command line: play a file and hear it change as you edit, render a file to WAV, run the engine as a server for your own clients, replay a session, and the language server. | You work from a terminal, render, or build on the engine. |

More than one client can play on the same engine at once: a client's notes belong to its own connection.

## Two halves of one file

A `.nml` file is a single language, but its two halves are read by different programs.

- **The server's half** — instruments, effects, samples, envelopes, functions, routing, modules. `niminal` compiles these into sound. This is most of the [language reference](/reference/language/).
- **The sequencer's half** — tempo, meter, clips, scenes, mini-notation patterns, arrangements. The sequencer (in Stage, and in `niminal play` and `niminal render`) reads these and turns them into notes with times; the engine never sees that text.

That is why every page of the language reference says which program runs what it describes: **Compiled by niminal**, **Read by the sequencer**, or **Read by both**. If you only want to write instruments and play notes from a program of your own, you can ignore the sequencer's half and use the [engine API](/reference/engine-api/) directly.

## Which one do I need?

| I want to… | Use |
| --- | --- |
| hear an instrument I just wrote | `niminal server`, and any client to play it |
| loop a pattern and change it while it plays | the [editor extension](/reference/editor/), or [Stage](/reference/stage/) |
| turn a file into a WAV | [`niminal render`](/guide/offline-rendering/) |
| play a file and hear it change as I save | [`niminal play`](/reference/server/cli/) |
| play notes from my own program | the [engine API](/reference/engine-api/) |
| replay a session a server recorded | [`niminal replay`](/reference/server/cli/) |
