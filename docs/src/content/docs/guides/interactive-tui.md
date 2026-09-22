---
title: Interactive TUI
description: Work in the terminal with queues, mentions, slash commands, and approval prompts.
---

With no prompt and a TTY on stdin and stdout, niminal opens a fullscreen terminal
UI. Each conversation is a session: the first message creates an append-only JSONL
file under `~/.niminal/sessions`, and later turns keep writing to it.

The keys below are defaults. You can [change TUI shortcuts](/reference/keybindings/)
in `~/.niminal/keybindings.json`.

## Sending messages

| Input | Action |
| --- | --- |
| Enter | Send, or accept a slash/file suggestion |
| Ctrl-D, Ctrl-S, Ctrl-Enter | Also send |
| Alt-J, Shift-Enter | Insert a newline |
| Alt-Left / Alt-Right | Move the composer cursor by word |
| Ctrl-Left / Ctrl-Right, Ctrl-A / Ctrl-E | Jump the composer cursor to the start or end of the draft |
| Esc | Interrupt a running turn, send queued messages now if any are waiting, or clear the composer when idle |
| Alt-Up, Shift-Left | Pop the last queued steering message back into the composer |
| Ctrl-C | Quit |
| Ctrl-G | Edit the composer in the configured `editor`, else `$VISUAL` or `$EDITOR` (`nano` if none is set) |

macOS Mission Control claims Ctrl-Left and Ctrl-Right before the terminal sees
them. Use Ctrl-A and Ctrl-E, or [bind the draft-jump actions](/reference/keybindings/)
to keys your terminal receives.

While a turn is running, Enter queues a **steering** message for the next model
request in that turn. Queued messages appear above the composer, and the footer
shows `queued N` when messages are waiting.

Press Esc while queued messages are waiting to interrupt the current turn and
send them immediately. Press Alt-Up or Shift-Left to move the last queued
message back into the composer for editing.

Follow-up queues exist in RPC mode and through extensions. The TUI itself only
queues steering messages while busy.

## Run a terminal command

Prefix a command with `!` to run it in the workspace and include its output in
the next model turn:

```text
!git status
```

Use `!!` when you want to run a command without sending its output to the model.
The command still appears in the transcript.

Output streams into a bash card as the command runs. Press Esc to interrupt.
Composer commands do not ask for approval: you typed the command yourself.

## History and suggestions

| Input | Action |
| --- | --- |
| Up / Down | Composer history when suggestions are closed |
| Up / Down | Move through suggestions when the list is open |
| Tab / Shift-Tab | Accept or cycle a suggestion |

Type `/` for slash command completion. Type `/settings` to open the config
overlay. Type `/model ` to pick from the models.dev
catalog for the active provider (filter from two characters). Type `/thinking `
to pick a supported level. Type `/theme ` to pick `light`, `dark`, or `auto`.
Type `/resume ` and Tab to pick a session. Type `/fork ` and Tab to pick a user
message to fork from.

Type `@` for file mention suggestions. Gitignored and hidden files are excluded.
Accepted mentions expand into attached file content in the outgoing message (up
to 100,000 bytes per attachment).

You can attach a PNG, JPEG, or WebP screenshot with Ctrl-V when your terminal
sends that shortcut to niminal. You can also type `@screenshot.png` or drop an
image file into the terminal. A drop pastes its file path, which niminal attaches
when you send the message. The composer shows attached filenames. Backspace with
an empty text draft removes the last attachment. You can send an image without
typing text.

Each image can be up to 10 MiB. Images are saved in the session, so a later turn
can still use them after the original file changes. A model must support image
input to inspect them. If Ctrl-V pastes text instead, your terminal may have
handled the shortcut before niminal received it. Use `@screenshot.png` or drop
the file in that case.

## Scrolling and copy

| Input | Action |
| --- | --- |
| Page Up / Page Down, mouse wheel | Scroll the transcript |
| Click a thinking, tool, or diff card | Expand or collapse it |
| Ctrl+O | Toggle the most recent thinking, tool, or diff card |
| Ctrl+Shift+O | Expand every card, or collapse all if they are already open |
| Drag select + release | Copy selection to clipboard |
| Ctrl-V, middle/right click | Attach a clipboard image, or paste text |
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

`read`, `grep`, `glob`, `ls`, `edit`, `write`, and `skill` run without prompting.
`bash` and extension tools normally ask first. Dangerous shell commands such as
`rm`, `sudo`, `curl`, and `git reset` are always re-prompted and cannot be
remembered.

Use `/yolo` or `--yolo` to auto-approve tools for the current process.

## Settings overlay

Run `/settings` to edit `~/.niminal/config.json` without leaving the TUI. The
overlay replaces the composer while it is open.

| Input | Action |
| --- | --- |
| Up / Down | Select a setting |
| Enter | Toggle a boolean, cycle an enum, or start editing text or numbers |
| Space | Toggle a boolean |
| Left / Right | Cycle enum settings |
| Enter while editing | Save the typed value |
| Esc while editing | Cancel the edit |
| Esc | Close the overlay |

Changes save immediately. Provider, model, thinking, and theme updates apply
to the current session right away.

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

You can add a theme by saving a JSON file in `~/.niminal/themes/`. The filename
is the theme name. For example, create `~/.niminal/themes/ocean.json`:

```json
{
  "base": "dark",
  "colors": {
    "accent": "#68c4d4",
    "code": "#9ed88f",
    "input_bg": "#1d3540"
  }
}
```

Run `/theme ocean` to apply it immediately and save your choice. Type `/theme `
to see available names, or select one in `/settings`. `base` is `light` or
`dark`; omitted colors use that palette. Colors must be `#RRGGBB`. Available
color names are `accent`, `code`, `add`, `del`, `meta`, `thinking`, `error`,
`muted`, `emphasis`, `italic`, `quote`, `input_fg`, `input_bg`, and `hover_bg`.
Edit the file and run `/theme ocean` again to reload it. If a theme file is
missing or invalid at startup, the TUI uses `auto` until you choose a theme.

Assistant replies render as Markdown: headings, lists, quotes, tables, inline
emphasis, and fenced code.

## Next steps

- [Keyboard shortcuts](/reference/keybindings/) for the full default map
- [Commands and shortcuts](/reference/commands/) for slash commands
- [Permissions](/guides/permissions/) for grants and YOLO mode
