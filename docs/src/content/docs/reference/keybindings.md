---
title: Keyboard shortcuts
description: Default TUI keybindings.
---

niminal ships fixed TUI keybindings. They are not rebindable through config today.

## Composer

| Key | Action |
| --- | --- |
| Enter | Send, or accept a slash/file suggestion |
| Ctrl-D, Ctrl-S, Shift-Enter (several escape variants) | Also send |
| Alt-J, Shift-Enter | Insert a newline |
| Up / Down | Composer history when suggestions are closed |
| Tab / Shift-Tab | Accept or cycle a suggestion |
| Esc | Interrupt a running turn, or clear the composer when idle |
| Ctrl-C | Quit |
| Ctrl-V, middle/right click | Paste into the composer |
| Ctrl-G | Edit the composer in the configured `editor`, else `$VISUAL` or `$EDITOR` (`nano` if none is set) |

While a turn is running, Enter queues a steering message.

## Transcript

| Key | Action |
| --- | --- |
| Page Up / Page Down | Scroll the transcript |
| Mouse wheel | Scroll the transcript |
| Click a thinking, tool, or diff card | Expand or collapse it |
| Ctrl+O | Toggle the most recent thinking, tool, or diff card |
| Ctrl+Shift+O | Expand every card, or collapse all if they are already open |
| Drag select + release | Copy selection to clipboard |

New output sticks to the bottom until you scroll up.

## Approval overlay

| Key | Action |
| --- | --- |
| Enter or `1` | Allow once |
| `s` | Allow for this session |
| `p` | Save a project grant (when allowed) |
| `n` or Esc | Deny |

## Next steps

- [Interactive TUI](/guides/interactive-tui/) for queues, mentions, and the footer
- [Commands and shortcuts](/reference/commands/) for slash commands
