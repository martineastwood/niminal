---
title: Interactive TUI
description: Work in the terminal with queues, mentions, slash commands, and approval prompts.
---

With no prompt and a TTY on stdin and stdout, niminal opens a fullscreen terminal
UI. Each conversation is a session: the first message creates an append-only JSONL
file under `~/.niminal/sessions`, and later turns keep writing to it.

## Sending messages

| Input | Action |
| --- | --- |
| Enter | Send, or accept a slash/file suggestion |
| Ctrl-D, Ctrl-S, Shift-Enter (several escape variants) | Also send |
| Alt-J, Shift-Enter | Insert a newline |
| Alt-Left / Alt-Right | Move the composer cursor by word |
| Ctrl-Left / Ctrl-Right, Ctrl-A / Ctrl-E | Jump the composer cursor to the start or end of the draft |
| Esc | Interrupt a running turn, send queued messages now if any are waiting, or clear the composer when idle |
| Alt-Up, Shift-Left | Pop the last queued steering message back into the composer |
| Ctrl-C | Quit |
| Ctrl-G | Edit the composer in the configured `editor`, else `$VISUAL` or `$EDITOR` (`nano` if none is set) |

macOS Mission Control claims Ctrl-Left and Ctrl-Right before the terminal sees
them. Uncheck "Move left a space" and "Move right a space" in
System Settings → Keyboard → Keyboard Shortcuts → Mission Control, or use
Ctrl-A and Ctrl-E instead.

While a turn is running, Enter queues a **steering** message for the next model
request in that turn. Queued messages appear above the composer, and the footer
shows `queued N` when messages are waiting.

Press Esc while queued messages are waiting to interrupt the current turn and
send them immediately. Press Alt-Up or Shift-Left to move the last queued
message back into the composer for editing.

Follow-up queues exist in RPC mode and through extensions. The TUI itself only
queues steering messages while busy.

## History and suggestions

| Input | Action |
| --- | --- |
| Up / Down | Composer history when suggestions are closed |
| Up / Down | Move through suggestions when the list is open |
| Tab / Shift-Tab | Accept or cycle a suggestion |

Type `/` for slash command completion. Type `/model ` to pick from the models.dev
catalog for the active provider (filter from two characters). Type `/thinking `
to pick a supported level. Type `/theme ` to pick `light`, `dark`, or `auto`.
Type `/resume ` and Tab to pick a session.

Type `@` for file mention suggestions. Gitignored and hidden files are excluded.
Accepted mentions expand into attached file content in the outgoing message (up
to 100,000 bytes per attachment).

## Scrolling and copy

| Input | Action |
| --- | --- |
| Page Up / Page Down, mouse wheel | Scroll the transcript |
| Click a thinking, tool, or diff card | Expand or collapse it |
| Ctrl+O | Toggle the most recent thinking, tool, or diff card |
| Ctrl+Shift+O | Expand every card, or collapse all if they are already open |
| Drag select + release | Copy selection to clipboard |
| Ctrl-V, middle/right click | Paste into the composer |
| `/copy` | Copy the last assistant reply or error |

New output sticks to the bottom until you scroll up.

## Approval overlay

When a tool needs approval, the TUI shows a prompt over the composer:

| Input | Action |
| --- | --- |
| Enter or `1` | Allow once |
| `s` | Allow for this session |
| `p` | Save a project grant (when allowed) |
| `n` or Esc | Deny |

`read`, `grep`, `glob`, `edit`, `write`, and `skill` run without prompting.
`bash` and extension tools normally ask first. Dangerous shell commands such as
`rm`, `sudo`, `curl`, and `git reset` are always re-prompted and cannot be
remembered.

Use `/yolo` or `--yolo` to auto-approve tools for the current process.

## Footer

The footer shows activity, extension status lines, token totals, the estimated
session cost from models.dev pricing, the active `provider/model`, the mapped
thinking level, and `[yolo]` when YOLO mode is on. Cost and token totals price
the whole session at the active model's rates.

Thinking, tool, and diff cards start compact in the transcript. Tool calls show
as a single line (`→ read README.md`). Shell commands show the command and a
short output preview; click to see the full result. Click a card, press Ctrl+O
to open the most recent one, or press Ctrl+Shift+O to expand or collapse every
card at once. Set `"show_thinking": true` in `~/.niminal/config.json` to start
new thinking cards expanded.

## Theme

Built-in palettes are `light`, `dark`, and `auto`. `/theme light`, `/theme dark`,
and `/theme auto` save the choice to your config. `auto` asks the terminal for
its background color at startup and falls back to dark when the query does not
answer.

Assistant replies render as Markdown: headings, lists, quotes, tables, inline
emphasis, and fenced code.

## Next steps

- [Keyboard shortcuts](/reference/keybindings/) for the full default map
- [Commands and shortcuts](/reference/commands/) for slash commands
- [Permissions](/guides/permissions/) for grants and YOLO mode
