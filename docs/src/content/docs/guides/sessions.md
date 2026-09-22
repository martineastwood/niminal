---
title: Sessions
description: Automatic saving, resume, fork, export, search, and recovery.
---

Every interactive or headless run with sessions enabled writes an append-only
JSONL file under `~/.niminal/sessions/`. The first line records the workspace
path; later lines record user messages, assistant replies, tool calls, tool
results, compaction summaries, and usage metadata.

Use `--no-session` to keep the transcript in memory only. That flag cannot be
combined with `--resume` or `--session`.

## Resume

```sh
niminal --resume                 # latest session for this workspace
niminal --session SESSION_ID     # one specific session
```

Inside the TUI:

| Command | Purpose |
| --- | --- |
| `/session` | Show the current session id and metadata |
| `/resume` | List the newest 20 sessions for this workspace |
| `/resume ID` | Load a session (shows a status note when the workspace differs) |
| `/search TEXT` | Search session text in this workspace |
| `/new` or `/clear` | Start a fresh session file |
| `/name [title]` | Show or set the session title |

Resuming restores the provider and model that session last used. It does not
change your saved defaults in `~/.niminal/config.json`.

## Fork, export, delete

| Command | Purpose |
| --- | --- |
| `/fork [N] [title]` | Copy the current session, or from user turn N, and switch to it |
| `/export [PATH]` | Export as Markdown, HTML (`.html`), or JSON (`.json`) |
| `/delete ID` | Move a session to the trash |
| `/restore` | List up to 20 deleted sessions |
| `/restore ID` | Restore one from the trash |

`/fork` with no number copies the whole session. `/fork 3` copies through user
turn 3, including that message and the assistant and tool replies that follow
it. The new session opens ready for your next message. Add an optional title
after the turn number, for example `/fork 3 retry parser`. In the TUI, type
`/fork ` and Tab to pick a user message from the list.

The default export path is `<session-id>.md` in the workspace. Deleted sessions
live under `~/.niminal/sessions/.trash/`.

## Recovery

If a session file ends with a truncated line, the next write keeps a
`.recovery-<timestamp>` copy of the original bytes and continues from the readable
prefix.

Interrupted tool calls are recorded as errors on load and are not rerun.

## Composer history

The TUI rebuilds composer history from user messages in the current session,
keeping the last 500 entries. History is not stored in a separate file.

## Next steps

- [Context and compaction](/guides/context-and-compaction/) when sessions grow long
- [Files and directories](/reference/files-and-directories/) for session paths
