---
title: Permissions
description: Which tools ask, how grants work, and when YOLO mode applies.
---

In the interactive TUI, niminal separates tools that can change your machine from
tools that only inspect the workspace.

## Silent by default

These built-in tools run without a prompt:

- `read`
- `grep`
- `glob`
- `ls`
- `git`
- `edit`
- `write`
- `skill`
- `ask_user`

## Prompted tools

These normally ask before they run:

- `bash` (model-initiated tool calls)
- Extension tools that are not read-only
- External tools that declare write, shell, or network capabilities

When prompted, press Enter for once, `s` for the session, `p` to save a project
grant, or `n` to deny.

Composer `!command` and `!!command` run without a prompt. You typed the
command yourself.

## Session modes

**Plan** mode limits the main agent to read-only built-in tools plus `git`. Those
calls run without prompts for the same reason as `read` and `grep`: they cannot
edit files or start shell commands through the tool interface. Plan mode also
includes `ask_user`, so the agent can question you about the design without
pausing for approval.

**Act** mode uses the normal rules above. Extension-registered modes can set
`permissions` to `read_only`, `ask`, or `yolo`; see
[Extensions and hooks](/guides/extensions-and-hooks/#register-a-session-mode).

Shift-Tab cycles modes in the TUI when idle. The choice is saved in the session
and restored on resume.

## Project grants

Project grants live in `<workspace>/.niminal/permissions.json`:

```json
{
  "allow": [
    "tool:my_extension_tool",
    "bash:npm test"
  ]
}
```

Keys use the form `tool:<name>` or `bash:<command>`. Grants load only when the
project is trusted. Use `/permissions` to list grants and `/permissions clear`
to remove them.

## Dangerous commands

These command words always re-prompt, even if you remembered an earlier grant:

`rm`, `git reset`, `git clean`, `git checkout --`, `git restore`, `sudo`, `curl`,
`wget`, `ssh`, `scp`, `chmod`, `chown`, `kill`, `pkill`, `dd`, `mkfs`, `shutdown`,
`reboot`

Matching looks for the word surrounded by spaces inside the full command string.

## YOLO mode

`/yolo` and `--yolo` skip all approval prompts in the interactive TUI for the
current process. Use `/yolo off` to turn it off again. The flag applies only
when launching the TUI; print, JSON, and RPC modes already run tools without
prompting.

## Headless modes

Print mode, JSON mode, and RPC mode have no approval UI. Tools run without
prompting there. Use them only with workspaces you trust, and narrow the tool
list with `--tools` when you can.

Approval is not a sandbox: shell commands still run as your user, with your
environment.

## Next steps

- [Security](/guides/security/) for trust and workspace boundaries
- [Built-in tools](/reference/tools/) for tool behavior and limits
