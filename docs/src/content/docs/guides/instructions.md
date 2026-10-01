---
title: Instructions
description: Project and personal rules from AGENTS.md and related files.
---

On each request niminal sends a stable prefix in this order:

1. The built-in system prompt, or a replacement from `SYSTEM.md` or `--system-prompt`
2. Optional append text from `APPEND_SYSTEM.md` or `--append-system-prompt`
3. Project instructions from `AGENTS.md`, `AGENTS.override.md`, or `CLAUDE.md`
4. The current session mode block (`act`, `plan`, or an extension-registered mode)

## Which files load globally

From `~/.niminal/`, niminal loads the first file that exists in this order:

1. `AGENTS.override.md`
2. `AGENTS.md`
3. `CLAUDE.md`

It then walks from the git root toward the workspace directory (less-specific
first) and loads the same filename precedence in each directory.

Each file is capped at 64 KiB. Larger files are truncated with an
`[instructions truncated]` marker.

Use `--no-context-files` or `-nc` to skip AGENTS and CLAUDE discovery for one
process. System prompt files and flags still apply.

## Write one with /init

Type `/init` in the composer and niminal writes the first version for you:

```text
/init
```

The agent reads your README, build manifests, CI workflows, and the instruction
files you already have. It runs the build and test commands it plans to document
before writing them down, so the result describes your project instead of a guess
at it. It writes `AGENTS.md` in the workspace root, and the next request in that
session picks it up.

When the file already exists, `/init` improves it in place. Lines that are still
true stay, wrong ones get fixed, and stale ones get dropped. Two cases are worth
knowing about:

- If the project has `CLAUDE.md` and no `AGENTS.md`, `/init` updates `CLAUDE.md`.
  An `AGENTS.md` beside it would win the precedence rule and quietly stop the old
  file from loading.
- If `AGENTS.override.md` exists, it keeps winning. The agent calls that out in
  its report.

`/init` writes a file, so it needs act mode. In plan mode it stops and tells you
to switch. Commit the result so the rest of the team gets it too.

## Replace or extend the system prompt

Replace the built-in identity with the first match:

1. `--system-prompt TEXT`
2. `<workspace>/.niminal/SYSTEM.md` when the project is trusted
3. `~/.niminal/SYSTEM.md`

Append extra rules with the first match:

1. `--append-system-prompt TEXT`
2. `<workspace>/.niminal/APPEND_SYSTEM.md` when the project is trusted
3. `~/.niminal/APPEND_SYSTEM.md`

Project instructions still load after the append text unless you pass
`--no-context-files`.

## Scoped instructions

Nested `AGENTS.md` files are **not** appended to every request. When you `read` a
path under a directory that has its own instruction file, niminal attaches that
scoped text once with the read output.

Use `/reload` after changing trusted project resources so discovery caches refresh.

## Trust

Project `AGENTS.md` and other instruction files in the git-root chain load
without trust. Project `SYSTEM.md` and `APPEND_SYSTEM.md` require trust, like
skills, prompts, tools, and extensions.

Portable files under `~/.niminal/` always load.

## Next steps

- [Skills](/guides/skills/) for on-demand procedure files
- [Prompt templates](/guides/prompt-templates/) for reusable slash commands
