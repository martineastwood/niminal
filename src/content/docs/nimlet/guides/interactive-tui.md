---
title: Interactive TUI
description: The composer, queued messages, transcript, shortcuts, and themes.
---

:::caution[Placeholder]
This page is a stub. Content is planned but not written yet.
:::

## Planned content

- The event-driven TUI and what "near-zero idle CPU" means in practice
  (`NIMTERM_PERF=1` for frame/latency diagnostics)
- Composer editing: Enter, Shift+Enter / Alt+Enter, cursor movement,
  Home/End, history
- Queueing the next message while a turn runs: what is committed when, and why
  slash commands cannot be queued
- Interrupting: Esc / Ctrl-C, Ctrl-U to clear and unqueue
- Transcript: scrolling (PgUp/PgDn, wheel), selecting and copying text,
  Ctrl-O to toggle tool output and thinking details, `/copy`
- Footer and `/stats`: model, context, token usage, cost
- Themes: `/theme`, built-ins (`auto`, `dark`, `light`), user themes in
  `.nimlet/themes` and `~/.nimlet/themes`
- Pasting text or clipboard images with Ctrl-V, and `@path` file/folder
  mentions in the composer
- Full keyboard shortcut table
