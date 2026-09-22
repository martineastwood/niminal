---
title: Skills
description: Load Markdown procedures with the skill tool or /skill:NAME.
---

Skills are directories that contain a `SKILL.md` file. niminal discovers them
recursively and exposes them to the model through the `skill` tool and
`/skill:NAME` slash commands. Supporting files can live beside `SKILL.md`.

## Where skills live

Global skills (always loaded):

```text
~/.agents/skills/<path>/<name>/SKILL.md
~/.niminal/skills/<path>/<name>/SKILL.md
```

Project skills (trusted workspace only; later roots win on name conflicts):

```text
<workspace>/.agent/skills/<path>/<name>/SKILL.md
<workspace>/.agents/skills/<path>/<name>/SKILL.md
<workspace>/.niminal/skills/<path>/<name>/SKILL.md
```

## Frontmatter

Optional YAML frontmatter can declare a description:

```markdown
---
description: Review pull requests for correctness and style.
---

Follow these steps when reviewing a PR...
```

Use `name` when the directory name is not the command name:

```markdown
---
name: review
description: Review pull requests for correctness and style.
---
```

The description appears in slash suggestions and in the `skill` tool listing.
When niminal loads a skill, it includes the skill directory so the model can
resolve relative paths such as `scripts/check.sh` and `references/rules.md`.

## Invoke a skill

In the TUI:

```text
/skill:review focus on error handling
```

The model can also call the `skill` tool with `{"name":"review"}` when the task
matches an available description.

Use `/reload` after adding or changing skills in a trusted project.

## Next steps

- [Built-in tools](/reference/tools/#skill) for the tool schema
- [Prompt templates](/guides/prompt-templates/) for fixed slash-command text
