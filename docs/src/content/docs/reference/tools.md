---
title: Built-in tools
description: The tools niminal always ships, their schemas, and their safety rules.
---

niminal gives the model a small set of tools for inspecting and changing the
workspace. The tool definitions are sent with each request.

## Tool list

| Tool | What it does | Approval in TUI |
| --- | --- | --- |
| `read` | Read a text file with numbered lines, or inspect an image | Auto |
| `grep` | Search file contents with a regex or plain text | Auto |
| `glob` | List workspace files matching a glob | Auto |
| `ls` | List one directory | Auto |
| `edit` | Replace exact text in one file | Auto |
| `write` | Create or replace a complete file | Auto |
| `bash` | Run a shell command in the workspace | Ask |
| `skill` | Load a discovered skill by name | Auto |

`--tools` can restrict the set, for example `--tools read,grep,glob` or
`--tools none`. The restriction applies to extension tools as well as built-ins.

Path-taking tools accept workspace-relative paths only. Symlink escapes outside
the workspace are rejected.

Independent read-only built-in calls (`read`, `grep`, `glob`, `ls`, `skill`) in the
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

For a PNG, JPEG, or WebP file, `read` sends the image to the model with its
filename. Images can be up to 10 MiB. Line ranges and edit version tokens apply
to text files only.

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

## `ls`

```json
{"path":"src/app"}
```

`path` is optional and defaults to the workspace root. One level is listed, not a
recursive walk: directories are suffixed with `/`. Results are capped at 200
entries, then `[truncated]` is appended. Use `glob` to match files by pattern
anywhere in the workspace.

`ls` reads the directory itself rather than the workspace file index, so it shows
what is really on disk: ignored entries such as `build/`, `.git/`, and
`node_modules/`, and directories that are still empty. `glob` and `grep` instead
use the index and honor `.gitignore`, so `ls` is how to discover those entries
before reading them directly.

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
Output is capped at 100,000 bytes. Carriage return overwrites the current line,
so progress bars do not accumulate.

In the TUI, output streams into the bash card as the command runs. Collapsed
cards show the first 8 lines. Expand the card to follow the full captured
output.

## `skill`

```json
{"name": "review"}
```

Loads `SKILL.md` for the named skill and returns its body plus the skill
directory, so the model can resolve supporting scripts and reference files.

## Next steps

- [Permissions](/guides/permissions/) for approval and grants
- [Skills](/guides/skills/) for authoring skill files
