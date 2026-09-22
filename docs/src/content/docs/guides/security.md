---
title: Security
description: Trust, permissions, workspace boundaries, and headless mode risks.
---

niminal is a coding agent that reads and edits files and can run shell commands
you approve. It is not a sandbox.

## Trust vs permissions

**Trust** decides whether project-local customizations load at all:

- `.niminal/permissions.json`
- Project skills, prompts, external tools, and extensions

**Permissions** decide whether an already-loaded tool may run without asking again.

Global resources under `~/.niminal/` and portable roots such as `~/.agents/`
always load. Project copies require trust.

## Trust prompt

The first interactive launch in a workspace that contains trusted resources asks
`[y/N]`. The answer is saved in `~/.niminal/trust.json` under
`projects.<canonical-workspace-path>`.

Use `/trust on` or `/trust off` to change it later. For one process, use
`--approve` or `--no-approve`.

Project `AGENTS.md` and other instruction files in the git-root chain load
without trust. Project `SYSTEM.md` and `APPEND_SYSTEM.md` require trust, like
the gated resources above.

## Workspace boundary

File tools (`read`, `grep`, `glob`, `ls`, `edit`, `write`) stay inside the current
working directory. Symlink escapes outside the workspace are rejected.

`grep` and `glob` use git-tracked and untracked files and honor `.gitignore`.

`bash` is not confined to the workspace. It runs as your user with your
environment. Treat shell approval as authorization, not isolation.

Composer `!command` and `!!command` also run as your user with no approval
prompt. Use them only for commands you intend to run.

## Credentials

API keys come from `--api-key`, `~/.niminal/auth.json`, or provider environment
variables. niminal does not write provider keys into the config file or session
files. If `auth.json` contains literal keys, make it readable only by your user:

```sh
chmod 600 ~/.niminal/auth.json
```

## Sessions

Sessions are append-only JSONL files under `~/.niminal/sessions/`. They contain
your prompts, model replies, tool calls, and tool output. Protect that directory
like any other local secret store.

## Network

niminal talks to the provider API you select. `/models refresh` fetches
`https://models.dev/api.json` and caches the catalog locally. Extensions and
external tools may add their own network use depending on what you install.

## Headless modes

Print, JSON, and RPC modes auto-execute tools. Do not point them at untrusted
project customizations unless you understand what those tools can do.

## Next steps

- [Permissions](/guides/permissions/) for grants and dangerous-command rules
- [External tools](/guides/external-tools/) for capability declarations
- [Extensions and hooks](/guides/extensions-and-hooks/) for long-running add-ons
