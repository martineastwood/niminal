---
title: Interactive TUI
description: The composer, queued messages, transcript, shortcuts, and themes.
---

Nimlet follows Pi's two-queue model:

- Enter queues a steering message, delivered after the current assistant tool
  batch and before the next model call.
- Alt+Enter queues a follow-up message, delivered after the agent finishes.
- Escape / Ctrl-C interrupts and restores queued messages to the composer.
- Alt+Up restores queued messages without interrupting the current turn.
- `/settings` opens the message-delivery settings: steering and follow-up can
  each use `one-at-a-time` or `all` delivery, persisted in the active config.

## Other controls

- The event-driven TUI and what "near-zero idle CPU" means in practice
  (`NIMTERM_PERF=1` for frame/latency diagnostics)
- Composer editing: Enter, Shift+Enter, cursor movement,
  Home/End, history
- Slash commands cannot be queued while a turn runs.
- Transcript: scrolling (PgUp/PgDn, wheel), selecting and copying text,
  Ctrl-O to toggle tool output and thinking details, `/copy`
- Footer and `/stats`: model, context, token usage, cost
- Themes: `/theme`, built-ins (`auto`, `dark`, `light`), user themes in
  `.nimlet/themes` and `~/.nimlet/themes`
- Pasting text or clipboard images with Ctrl-V, and `@path` file/folder
  mentions in the composer
- Full keyboard shortcut table
