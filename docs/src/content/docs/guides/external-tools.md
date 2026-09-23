---
title: External tools
description: Expose a one-shot executable as a typed model tool with tool.json.
---

External tools are short-lived executables described by `tool.json`. niminal starts
one only when the model calls it, sends the tool arguments as one JSON object on
stdin, and expects one JSON value on stdout.

## Manifest

Put a tool in a directory with a `tool.json` beside the executable:

```json
{
  "name": "word_count",
  "description": "Count words in text.",
  "command": ["./word-count"],
  "input_schema": {
    "type": "object",
    "properties": {"text": {"type": "string"}},
    "required": ["text"]
  },
  "timeout_seconds": 10,
  "capabilities": ["read"]
}
```

Required fields: `name`, `description`, `command`, and `input_schema`.

`command` must be a non-empty string array. The first item resolves relative to
the manifest directory; later items are fixed arguments. The process runs in the
workspace.

`timeout_seconds` defaults to 30. Output is capped at 100,000 bytes.

## Capabilities

Supported values: `read`, `write`, `shell`, `network`, and `user`.

Tools that declare only `read` and optionally `user` are treated as read-only
and auto-approved in the TUI like built-in read tools.

## Discovery paths

Global tools (always loaded; later paths win on name conflicts):

```text
~/.agents/tools/NAME/tool.json
~/.niminal/tools/NAME/tool.json
```

Project tools (trusted workspace only):

```text
<workspace>/.agent/tools/NAME/tool.json
<workspace>/.agents/tools/NAME/tool.json
<workspace>/.niminal/tools/NAME/tool.json
```

Names that collide with built-in tools are skipped with a warning. Use `/reload`
after changing a manifest.

## Next steps

- [Permissions](/guides/permissions/) for approval behavior
- [Extensions and hooks](/guides/extensions-and-hooks/) for long-running programs
