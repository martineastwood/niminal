---
title: Built-in tools
description: The tools niminal always ships, their schemas, and their safety rules.
---

niminal gives the model a small set of tools for inspecting and changing the
workspace. The tool definitions are sent with each request.

## Tool list

| Tool | What it does | Approval in TUI |
| --- | --- | --- |
| `read` | Read a text file with numbered lines and a version token | Auto |
| `grep` | Search file contents with a regex or plain text | Auto |
| `glob` | List workspace files matching a glob | Auto |
| `edit` | Replace exact text in one file | Auto |
| `write` | Create or replace a complete file | Auto |
| `bash` | Run a shell command in the workspace | Ask |
| `skill` | Load a discovered skill by name | Auto |

`--tools` can restrict the set, for example `--tools read,grep,glob` or
`--tools none`. The restriction applies to extension tools as well as built-ins.

Path-taking tools accept workspace-relative paths only. Symlink escapes outside
the workspace are rejected.

Independent read-only built-in calls (`read`, `grep`, `glob`, `skill`) in the
same model response can run in parallel. Other tools run one at a time in request
order.

## `read`

```json
{
  "path": "src/parser.cpp",
  "start_line": 1,
  "end_line": 80
}
```

`path` is required. Line numbers are one-based and the end is inclusive. Omit a
range to read the whole file. Text is returned with numbered lines, a
workspace-relative path, and a `version` token for `edit.expected_version`.

Text output is limited to 200,000 bytes. Scoped `AGENTS.md` instructions for the
directory are appended when present.

niminal returns text only from `read`; image attachments are not supported by
this tool today.

## `grep`

```json
{
  "pattern": "TODO|FIXME",
  "glob": "**/*.cpp",
  "path": "src",
  "case_insensitive": true,
  "max_matches": 80
}
```

`pattern` is required. It is an ECMAScript regular expression; plain text works
too. Use `glob` to filter file names and `path` to limit the search to a workspace
subdirectory.

The default match limit is 80 and the maximum is 200. Binary files and files larger
than 1 MiB are skipped. No matches returns `No matches.`

## `glob`

```json
{"pattern":"src/**/*.cpp","path":"."}
```

`pattern` is required. Results are capped at 200 files. Listing respects
`.gitignore` through the workspace file index.

## `edit`

```json
{
  "path": "src/parser.cpp",
  "old_text": "return false;",
  "new_text": "return true;",
  "expected_version": "abc123"
}
```

`old_text` must match exactly once. Pass `expected_version` from the latest
`read` output to detect concurrent changes.

## `write`

```json
{
  "path": "notes.txt",
  "content": "hello",
  "overwrite": true
}
```

Creates a new file by default. Existing files require `"overwrite": true`.

## `bash`

```json
{
  "command": "npm test",
  "timeout_seconds": 120
}
```

Runs in the workspace directory. Default timeout is 120 seconds; allowed range is
1 through 600. Combined stdout and stderr are returned with an exit code line.
Output is capped at 100,000 bytes.

## `skill`

```json
{"name": "review"}
```

Loads `SKILL.md` for the named skill and returns its body to the model.

## Next steps

- [Permissions](/guides/permissions/) for approval and grants
- [Skills](/guides/skills/) for authoring skill files
