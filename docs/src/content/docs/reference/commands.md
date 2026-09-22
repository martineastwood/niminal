---
title: Commands and shortcuts
description: Every slash command and CLI flag.
---

Slash commands are typed in the composer and start with `/`. They configure
niminal, manage sessions, or expand prompt templates. They do not become model
messages except for template expansion and `/skill:NAME`.

`/help` prints the command list inside the app. The keyboard shortcuts live on
the [Keyboard shortcuts](/reference/keybindings/) page.

## Built-in slash commands

| Command | Description |
| --- | --- |
| `/help` | Show command list and keybinding help |
| `/version` | Show the version |
| `/provider [name]` | Show or set the provider |
| `/model [ID]` | Show or set the model |
| `/thinking [level]` | Show or set reasoning |
| `/theme [light\|dark\|auto]` | Show or set theme |
| `/permissions [clear]` | Show or clear tool grants |
| `/trust [on\|off]` | Show or set project resource trust |
| `/yolo [off]` | Auto-approve tools for this process |
| `/models refresh` | Refresh the models.dev catalog |
| `/session` | Show the current session |
| `/name [title]` | Show or set the session name |
| `/resume [ID]` | List or load a session |
| `/search TEXT` | Search sessions for text |
| `/fork [N] [title]` | Copy this session, or from user turn N |
| `/export [PATH]` | Write this session as Markdown, HTML, or JSON |
| `/delete ID` | Move a session to the trash |
| `/restore [ID]` | List or restore a deleted session |
| `/new` | Start a new session |
| `/clear` | Same as `/new` |
| `/copy` | Copy the last error or reply |
| `/retry` | Retry the last failed request |
| `/compact [instruction]` | Summarize older session history |
| `/reload` | Reload trusted project resources |
| `/skill:NAME [request]` | Load a skill |
| `/quit`, `/exit` | Exit |

## Dynamic commands

Dispatch order:

1. Built-in commands
2. Extension-registered commands
3. Prompt templates (`/NAME [args]`)
4. `/skill:NAME [request]`

Built-in names cannot be overridden by templates.

## `@` mentions

Type `@` in the composer to attach workspace files to the outgoing message.
Gitignored and hidden files are excluded from suggestions.

## `!` shell commands

| Prefix | Behavior |
| --- | --- |
| `!command` | Run in the workspace and include output in the next model turn |
| `!!command` | Run in the workspace without sending output to the model |

Examples: `!git status`, `!!npm test`

These run in the TUI only. They do not start a model turn and do not show an
approval prompt.

## CLI flags {#cli-flags}

| Flag | Purpose |
| --- | --- |
| `--help`, `-h` | Show usage |
| `--version`, `-v` | Print the version and exit |
| `--model ID` | Model for this run |
| `--provider NAME` | Provider for this run |
| `--thinking LEVEL` | Thinking level for this run |
| `--mode json` | Emit versioned JSONL events and exit |
| `--mode rpc` | Serve JSONL commands until shutdown or EOF |
| `--api-key KEY` | API key for this process |
| `--tools LIST` | Comma-separated tool names, or `none` |
| `--max-steps N` | Tool loop cap (`0` means unlimited) |
| `--yolo` | Auto-approve tools for this process |
| `--approve` | Load project-local resources without trust prompt |
| `--no-approve` | Skip project-local resources |
| `--resume` | Resume latest session for this workspace |
| `--session ID` | Resume a specific session |
| `--no-session` | Keep transcript in memory only |
| `--system-prompt TEXT` | Replace the built-in system prompt for this run |
| `--append-system-prompt TEXT` | Append to the system prompt for this run |
| `--no-context-files`, `-nc` | Skip `AGENTS.md` and `CLAUDE.md` discovery |
| `--` | End of flags; remainder is prompt |

## Invocation modes

| Invocation | Behavior |
| --- | --- |
| `niminal` | Interactive TUI when stdin and stdout are TTYs |
| `niminal prompt…` | One print-mode turn, then exit |
| `niminal --mode json …` | JSONL on stdout, then exit |
| `niminal --mode rpc` | JSON commands on stdin until `shutdown` or EOF |

Piped stdin is merged with a CLI prompt when stdin is not a TTY.

`--no-session` cannot be combined with `--resume` or `--session`.

## Next steps

- [Keyboard shortcuts](/reference/keybindings/)
- [JSON mode](/reference/json-mode/)
- [RPC mode](/reference/rpc-mode/)
