---
title: Instructions
description: Project and personal rules from AGENTS.md and related files.
---

On each request niminal sends a stable prefix: the built-in system prompt, then
project instructions, then an ACT mode block that tells the model to implement
and verify changes.

## Which files load globally

From `~/.niminal/`, niminal loads the first file that exists in this order:

1. `AGENTS.override.md`
2. `AGENTS.md`
3. `CLAUDE.md`

It then walks from the git root toward the workspace directory (less-specific
first) and loads the same filename precedence in each directory.

Each file is capped at 64 KiB. Larger files are truncated with an
`[instructions truncated]` marker.

## Scoped instructions

Nested `AGENTS.md` files are **not** appended to every request. When you `read` a
path under a directory that has its own instruction file, niminal attaches that
scoped text once with the read output.

Use `/reload` after changing trusted project resources so discovery caches refresh.

## Trust

Project instruction files in the workspace load without a trust prompt. Trust only
gates project permissions, skills, prompts, tools, and extensions.

Portable instruction files under `~/.niminal/` always load.

## Next steps

- [Skills](/guides/skills/) for on-demand procedure files
- [Prompt templates](/guides/prompt-templates/) for reusable slash commands
