---
title: Quickstart
description: Set an API key and run your first turn with niminal.
---

This guide assumes the `niminal` binary is already installed and available on
your `PATH`. If it is not, follow [Install](/guides/install/) first. Give it a
provider credential, then run it from the workspace you want the agent to inspect
or change.

## Configure provider access

```sh
export OPENROUTER_API_KEY=your-key
```

OpenRouter is the default provider. You can use another wired provider by
exporting its key, then selecting it with `--provider` or `/provider`:

| Provider | Credential environment variable |
| --- | --- |
| Anthropic | `ANTHROPIC_API_KEY` |
| Google Gemini API | `GEMINI_API_KEY`, `GOOGLE_API_KEY`, or `GOOGLE_GENERATIVE_AI_API_KEY` |
| Hyper | `HYPER_API_KEY` |
| Mistral | `MISTRAL_API_KEY` |
| OpenAI | `OPENAI_API_KEY` |
| OpenCode Go or Zen | `OPENCODE_API_KEY` |
| OpenRouter | `OPENROUTER_API_KEY` |

For a custom environment variable, add a provider entry to
`~/.niminal/auth.json`:

```json title="~/.niminal/auth.json"
{
  "openai": {
    "key": "$MY_OPENAI_KEY"
  }
}
```

The `key` can also be a literal API key. Keep this file private:

```sh
chmod 600 ~/.niminal/auth.json
```

You can pass `--api-key KEY` for a one-process override instead of exporting a
variable. Key resolution is `--api-key`, then `auth.json`, then the provider's
standard environment variable. See [Configuration](/guides/configuration/) for
the full config file.

Codex App Server is not wired in niminal today.

## Start your first turn

Run niminal from the project directory:

```sh
cd /path/to/your/project
niminal
```

Try a small request:

```text
Explain how this project runs its tests, then suggest the smallest useful fix
for the failing parser test.
```

The interactive footer shows the active provider, model, thinking level, token
totals, and the estimated session cost. File reads, searches, and workspace edits
are built in. Shell commands normally ask for approval the first time.

## One-shot and piped input

Pass a prompt after the flags for a turn that exits when it finishes:

```sh
niminal fix the failing parser test
```

Print mode uses only the CLI prompt. To pipe file content into a one-shot run,
use JSON mode:

```sh
cat README.md | niminal --mode json summarize this
```

Diagnostics go to stderr. Use `--no-session` for a run that is not written to
the session directory.

## Resume a session

Sessions are saved automatically. Pick up where you left off:

```sh
niminal --resume                      # latest session for this workspace
niminal --session 1789233281025102    # one specific session
```

Inside the TUI, `/resume` lists recent sessions for this workspace and `/resume
ID` loads one. Tab completes session ids. `/new` starts fresh. See
[Sessions](/guides/sessions/) for naming, forking, export, and recovery.

## Flags for scripts and CI

These flags apply to one process only and are never written to your config:

| Flag | Purpose |
| --- | --- |
| `--provider NAME` | Provider for this run |
| `--model ID` | Model for this run |
| `--thinking LEVEL` | Thinking level for this run |
| `--api-key KEY` | In-memory API key override |
| `--tools LIST` | Restrict tools (`read,grep,glob` or `none`) |
| `--max-steps N` | Tool loop cap (`0` means unlimited) |
| `--yolo` | Auto-approve all tools in the interactive TUI |
| `--approve` | Load project customizations without the trust prompt |
| `--no-approve` | Skip project customizations |
| `--system-prompt TEXT` | Replace the built-in system prompt for this run |
| `--append-system-prompt TEXT` | Append to the system prompt for this run |
| `--no-context-files`, `-nc` | Skip `AGENTS.md` and `CLAUDE.md` discovery |
| `--mode json` | Emit versioned JSONL events and exit |
| `--mode rpc` | Serve JSONL commands until shutdown or EOF |
| `--version` | Print the version and exit |

Print, JSON, and RPC modes have no approval UI, so tools run without prompting.
Narrow the tool list when you can:

```sh
niminal "summarize the README" --tools read
```

The full flag list is in [Commands and shortcuts](/reference/commands/#cli-flags).

## Project trust

The first time you open a repository that ships niminal customizations (project
permissions, skills, prompts, tools, or extensions), niminal asks whether to
load them. The default is no. Answer yes with `/trust on`, or for one run with
`--approve`. See [Security](/guides/security/) for what trust covers.

## Next steps

- [Interactive TUI](/guides/interactive-tui/) for queues, mentions, and keybindings
- [Models and providers](/guides/models-and-providers/) to switch models
- [Commands and shortcuts](/reference/commands/) for the complete command list
