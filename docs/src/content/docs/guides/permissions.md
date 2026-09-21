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
- `edit`
- `write`
- `skill`

## Prompted tools

These normally ask before they run:

- `bash`
- Extension tools that are not read-only
- External tools that declare write, shell, or network capabilities

When prompted, press Enter for once, `s` for the session, `p` to save a project
grant, or `n` to deny.

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

`/yolo` and `--yolo` skip all approval prompts for the current process. Use
`/yolo off` to turn it off again in the TUI.

## Headless modes

Print mode, JSON mode, and RPC mode have no approval UI. Tools run without
prompting there. Use them only with workspaces you trust, and narrow the tool
list with `--tools` when you can.

Approval is not a sandbox: shell commands still run as your user, with your
environment.

## Next steps

- [Security](/guides/security/) for trust and workspace boundaries
- [Built-in tools](/reference/tools/) for tool behavior and limits
