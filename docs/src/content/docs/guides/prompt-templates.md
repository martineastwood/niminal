---
title: Prompt templates
description: Turn Markdown files into slash commands with $ARGUMENTS substitution.
---

Prompt templates are flat Markdown files that become bare slash commands. Put
them in one of these folders (later folders win when names collide):

```text
~/.agents/prompts/
~/.niminal/prompts/
<workspace>/.agent/prompts/
<workspace>/.agents/prompts/
<workspace>/.niminal/prompts/
```

Project prompt folders load only when the workspace is trusted.

## Example

Save `.niminal/prompts/review.md`:

```markdown
---
description: Review the current changes.
---

Review the current diff for correctness and style.
Focus on: $ARGUMENTS
```

Type `/review parser edge cases` in the TUI. niminal sends the expanded body,
with `$ARGUMENTS` or `$@` replaced by the text after `/review`.

## Rules

- Only `*.md` files directly inside those folders are discovered (no subfolders)
- Files over 100,000 bytes are ignored
- Built-in slash commands keep their names; templates cannot override them
- Optional frontmatter `description` feeds Tab completion; without it, the first
  useful body line is used

Templates become user messages to the model, not local commands.

## Next steps

- [Commands and shortcuts](/reference/commands/) for dispatch order
- [Interactive TUI](/guides/interactive-tui/) for Tab completion behavior
