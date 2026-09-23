---
title: Keyboard shortcuts
description: Default TUI keybindings and how to change them.
---

You can change niminal's TUI shortcuts in `~/.niminal/keybindings.json`. Each
action accepts one key or a list of keys. For example, to use Ctrl-B and Ctrl-F
to jump to the start and end of your draft:

```json title="~/.niminal/keybindings.json"
{
  "composer.draftStart": "ctrl+b",
  "composer.draftEnd": "ctrl+f"
}
```

Create the file if it does not exist, then run `/reload` to apply your changes.
You only need to list actions you want to change. Each entry replaces that
action's default keys. `/help` shows the keys active in the current TUI.

Use lowercase key names. Join modifiers with `+`, as in `alt+left` or
`ctrl+shift+o`. Supported modifiers are `ctrl`, `alt`, and `shift`. You can use
letters, digits, `enter`, `escape`, `tab`, `up`, `down`, `left`, `right`, `home`,
`end`, `pageup`, `pagedown`, `backspace`, and `delete`. A key combination only
works when your terminal sends it to niminal as a distinguishable key event.
Ctrl-I, Ctrl-J, and Ctrl-M cannot be bound separately from Tab or Enter and are
rejected in this file.

If the file contains an unsupported key, unknown action, or two shortcuts that
conflict in the same TUI context, niminal shows the error at startup or on
`/reload` and uses the default keybindings. Every action needs at least one
key.
Mouse actions and ordinary composer editing keys are fixed. A custom shortcut
using a key such as Backspace or Left takes precedence over its editing action.

## Composer

| Action | Default keys | What it does |
| --- | --- | --- |
| `composer.submit` | Enter, Ctrl-D, Ctrl-S, Ctrl-Enter | Send, queue while a turn runs, or accept a slash/file suggestion |
| `composer.newline` | Alt-J, Shift-Enter | Insert a newline |
| `composer.wordLeft` / `composer.wordRight` | Alt-Left / Alt-Right | Move the cursor by word |
| `composer.draftStart` / `composer.draftEnd` | Ctrl-Left or Ctrl-A / Ctrl-Right or Ctrl-E | Jump to the start or end of the draft |
| `composer.previous` / `composer.next` | Up / Down | Move through suggestions, or through history when suggestions are closed |
| `composer.complete` / `composer.completePrevious` | Tab / Shift-Tab | Accept a suggestion, or cycle back and accept it |
| `composer.cancel` | Esc | Interrupt a turn, send queued messages now, or clear an idle composer |
| `composer.editQueued` | Alt-Up, Shift-Left | Pop the last queued steering message back into the composer |
| `composer.paste` | Ctrl-V | Attach a clipboard image, or paste text |
| `composer.externalEditor` | Ctrl-G | Edit the composer in the configured `editor`, else `$VISUAL` or `$EDITOR` (`nano` if none is set) |
| `app.quit` | Ctrl-C | Quit |

macOS Mission Control claims Ctrl-Left and Ctrl-Right before the terminal sees
them. You can use the default Ctrl-A and Ctrl-E keys, or bind `composer.draftStart`
and `composer.draftEnd` to keys your terminal receives.

While a turn is running, submitting queues a steering message. Queued messages
appear above the composer. `composer.cancel` sends them immediately, and
`composer.editQueued` returns the last one to the composer.

## Transcript

| Action | Default keys | What it does |
| --- | --- | --- |
| `transcript.scrollUp` / `transcript.scrollDown` | Page Up / Page Down | Scroll the transcript |
| `transcript.toggleLast` | Ctrl-O | Toggle the most recent thinking, tool, or diff card |
| `transcript.toggleAll` | Ctrl-Shift-O | Expand every card, or collapse all if already open |

The mouse wheel also scrolls. Click a card to expand or collapse it, or drag
select and release to copy text. New output sticks to the bottom until you
scroll up.

## Approval prompt

| Action | Default keys | What it does |
| --- | --- | --- |
| `approval.allowOnce` | Enter, `1` | Allow once |
| `approval.allowSession` | `s` | Allow for this session |
| `approval.allowProject` | `p` | Save a project grant when allowed |
| `approval.deny` | `n`, Esc | Deny |

Approval keys only act while an approval prompt is open. For example, Enter
can submit a message in the composer and allow once in the approval prompt.

## Next steps

- [Interactive TUI](/guides/interactive-tui/) for queues, mentions, and the footer
- [Configuration](/guides/configuration/) for other global settings
