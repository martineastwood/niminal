---
title: Files and directories
description: Every path niminal reads and writes on your machine.
---

## Global paths

| Path | Purpose |
| --- | --- |
| `~/.niminal/config.json` | User settings |
| `~/.niminal/auth.json` | Provider credentials |
| `~/.niminal/keybindings.json` | Interactive TUI shortcuts |
| `~/.niminal/themes/*.json` | Custom TUI themes |
| `~/.niminal/sessions/` | Append-only session JSONL files |
| `~/.niminal/sessions/.trash/` | Deleted sessions |
| `~/.niminal/trust.json` | Per-workspace trust decisions |
| `~/.niminal/models-dev.json` | Cached models.dev catalog |
| `~/.niminal/skills/<name>/SKILL.md` | Global skills |
| `~/.niminal/prompts/*.md` | Global prompt templates |
| `~/.niminal/tools/<name>/tool.json` | Global external tools |
| `~/.niminal/extensions/<name>/extension.json` | Global extensions |
| `~/.niminal/AGENTS.md`, `AGENTS.override.md`, `CLAUDE.md` | Global instructions |
| `~/.niminal/SYSTEM.md` | Global system prompt replacement |
| `~/.niminal/APPEND_SYSTEM.md` | Global system prompt append |

Portable global roots shared with other agents:

| Path | Purpose |
| --- | --- |
| `~/.agents/skills/` | Portable global skills |
| `~/.agents/prompts/` | Global prompt templates |
| `~/.agents/tools/` | Global external tools |
| `~/.agents/extensions/` | Global extensions |

Later discovery roots win on name conflicts for skills, prompts, and external tools.
Extensions do not override by name; every matching extension starts.

## Project paths (trusted workspace)

| Path | Purpose |
| --- | --- |
| `<workspace>/.niminal/permissions.json` | Project tool grants |
| `<workspace>/.niminal/skills/` | Project skills |
| `<workspace>/.niminal/prompts/` | Project prompt templates |
| `<workspace>/.niminal/tools/` | Project external tools |
| `<workspace>/.niminal/extensions/` | Project extensions |
| `<workspace>/.agent/`, `.agents/` | Alternate project roots for skills, prompts, tools |
| `<workspace>/AGENTS.md`, `AGENTS.override.md`, `CLAUDE.md` | Project instructions |
| `<workspace>/.niminal/SYSTEM.md` | Project system prompt replacement |
| `<workspace>/.niminal/APPEND_SYSTEM.md` | Project system prompt append |

Project instructions load without trust. `SYSTEM.md`, `APPEND_SYSTEM.md`,
permissions, skills, prompts, tools, and extensions require trust (or
`--approve` for one process).

## Session files

Session files are `<microsecond-id>.jsonl` directly under `~/.niminal/sessions/`.
The first record identifies the workspace. Recovery backups use the suffix
`.recovery-<timestamp>` beside the original file.

## Config writes

`/provider`, `/model`, `/thinking`, and `/theme` always write
`~/.niminal/config.json`. RPC `set_steering_mode` and `set_follow_up_mode` write
there too.

There is no project-level config overlay file.

## Next steps

- [Configuration](/guides/configuration/) for config keys
- [Security](/guides/security/) for trust behavior
