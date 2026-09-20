# niminal

A local coding agent as a single native binary. You describe a change, niminal
reads and edits the workspace, and streams the turn in your terminal.

This is the C++ niminal tree: a small AI library plus the coding-agent app
(print mode and a TUI).

## Prerequisites

- CMake 3.22 or later
- a C++20 compiler
- OpenSSL 3 development libraries (static `.a` files)

On macOS:

```sh
brew install cmake openssl@3
```

On Debian or Ubuntu:

```sh
sudo apt install cmake g++ libssl-dev zlib1g-dev
```

## Build

```sh
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build
```

The binary is `build/niminal`. Curl and nlohmann/json are fetched during
configure. OpenSSL is static-linked from the copy CMake found at build time.

## Run

```sh
export OPENROUTER_API_KEY=your-key
cd /path/to/your/project
/path/to/niminal/build/niminal
```

OpenRouter is the default. Switch with `--provider NAME` or `/provider NAME`,
and export the matching key:

| Provider | Default model | Key | Native endpoint |
| --- | --- | --- | --- |
| `openrouter` | `openai/gpt-4o-mini` | `OPENROUTER_API_KEY` | `https://openrouter.ai/api/v1/chat/completions` |
| `anthropic` | `claude-sonnet-4-6` | `ANTHROPIC_API_KEY` | `https://api.anthropic.com/v1/messages` |
| `google` | `gemini-3.5-flash-lite` | `GEMINI_API_KEY` | `https://generativelanguage.googleapis.com/v1beta` |
| `openai` | `gpt-5` | `OPENAI_API_KEY` | `https://api.openai.com/v1/chat/completions` |
| `hyper` | `deepseek-v4-flash` | `HYPER_API_KEY` | `https://hyper.charm.land/v1/chat/completions` |
| `mistral` | `mistral-vibe-cli-with-tools` | `MISTRAL_API_KEY` | `https://api.mistral.ai/v1/chat/completions` |
| `opencode` | `deepseek-v4.1-flash` | `OPENCODE_API_KEY` | `https://opencode.ai/zen/go/v1/chat/completions` |
| `opencodezen` | `deepseek-v4-flash` | `OPENCODE_API_KEY` | `https://opencode.ai/zen/v1/chat/completions` |

`google` also accepts `GOOGLE_API_KEY` or `GOOGLE_GENERATIVE_AI_API_KEY`.
Switching provider restores that provider's last model from
`~/.niminal/config.json`. `/provider` alone prints the active name, model,
endpoint, and which env var supplies the key.

The agent always speaks one request shape: OpenAI-style messages and tools.
niminal translates that to the provider's native API before it goes on the
wire, including prompt cache:

- Anthropic: Messages API with `cache_control` on the last tool, last system
  block, and last message
- OpenRouter and OpenCode Zen: the same breakpoints, plus a session cache key
- OpenAI and Mistral: a stable prefix and `prompt_cache_key`
- Google: native `generateContent`, which caches a repeated prefix automatically
- Hyper and OpenCode Go: the same stable prefix on Chat Completions

Codex App Server is not wired.

With no prompt, niminal opens a fullscreen TUI. Each conversation is a session:
the first message creates an append-only JSONL file under
`~/.niminal/sessions`, and later turns keep writing to it.

- Enter sends. If a turn is already running, Enter queues a steering message
  for the next model request in that turn.
- Up/Down walk the current session's composer history when slash suggestions are
  not open.
- Trackpad or mouse wheel scrolls the transcript. Page Up / Page Down jump
  further. New output sticks to the bottom until you scroll up.
- Drag to copy selected text. Ctrl-V pastes into the composer. `/copy` copies
  the last assistant reply or error.
- Alt-J or Shift-Enter inserts a newline
- Esc interrupts a running turn, or clears the composer when idle
- Ctrl-C quits
- `/help`, `/provider`, `/model`, `/thinking`, `/models refresh`, `/session`, `/name`, `/resume`,
  `/new`, `/clear`, `/copy`, `/compact`, `/skill:NAME`, `/NAME`, `/quit`

- Tab completes a slash command. Up/Down moves through the list. Type `/model `
  to pick from the models.dev catalog for the active provider (filter from two
  characters). Type `/thinking ` to pick a level the current model supports.
  Type `/resume ` and Tab to pick a session.

Skills are `SKILL.md` files under `~/.niminal/skills/<name>/` or the current
workspace's `.niminal/skills/<name>/` (workspace skills win on name conflicts).
The optional frontmatter `description` appears in slash suggestions and tells
the model when to load the skill itself. Invoke one directly with
`/skill:NAME optional request`.

Prompt templates are Markdown files that become bare slash commands. Put them in
`~/.agents/prompts/`, `~/.niminal/prompts/`, or one of the workspace folders
`.agent/prompts/`, `.agents/prompts/`, and `.niminal/prompts/`. Later folders win
when names collide. For example, save this as `.niminal/prompts/review.md`:

```markdown
---
description: Review the current changes.
---

Review the current diff for correctness and style.
Focus on: $ARGUMENTS
```

Type `/review parser edge cases` in the TUI. niminal sends the expanded body,
with `$ARGUMENTS` or `$@` replaced by the text after `/review`. The first useful
body line is used as the suggestion description when frontmatter is omitted.
Templates are flat, only `*.md` files directly inside those folders are found,
and files over 100,000 bytes are ignored. Built-in commands keep their names.

`/model ID`, `/provider NAME`, and `/thinking LEVEL` are saved to
`~/.niminal/config.json`. The thinking value is a shared ladder
(`none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`). When you switch
models, niminal keeps that saved level and maps it to whatever the new model
actually accepts, using models.dev `reasoning` / `reasoning_options` plus
Anthropic's adaptive effort names. `/thinking` with no argument prints the
mapped level. Unset thinking leaves the provider default.

On startup, niminal loads the config file, then applies `NIMINAL_MODEL` /
`NIMINAL_API_URL` / `NIMINAL_THINKING` if they are set, then `--provider`,
`--model`, and `--thinking`.

Resume the latest session for this directory with `--resume`, or a specific
file with `--session ID`. `/resume` lists the newest 20 sessions that belong to
the current workspace. `/resume ID` loads one, including sessions started
somewhere else (those show a workspace warning). `/new` starts a fresh file;
the old one stays on disk. `/clear` does the same. `--no-session` keeps the
transcript in memory only, and cannot be combined with `--resume` or
`--session`.

Resuming restores the provider and model that session last used. It does not
change your saved defaults. Interrupted tool calls are recorded as errors on load
and are not rerun. If the last line of a file is truncated, the next write keeps a
`.recovery-*` copy of the original bytes and continues from the readable
prefix.

When a resumed session is too large for the model, niminal summarizes older
turns, keeps recent ones verbatim, and retries. `/compact` does the same on
demand. The JSONL file still holds the full history. The model only sees the
latest summary plus the kept tail.

Print mode still runs one turn and exits:

```sh
/path/to/niminal/build/niminal "Explain what this repo does in one paragraph."
```

Optional environment:

- `NIMINAL_MODEL` (overrides `~/.niminal/config.json`)
- `NIMINAL_API_URL` (overrides the config file, default OpenRouter chat completions)
- `NIMINAL_THINKING` (overrides `thinking` in the config file)

Optional flags: `--model ID`, `--provider NAME`, `--thinking LEVEL`, `--max-steps N`,
`--resume`, `--session ID`, `--no-session`.

File tools stay inside the current directory. `grep` and `glob` use git's
tracked and untracked files and honor `.gitignore`, so `build/` stays out of
search. `bash` runs unprompted with that directory as cwd. There is no
permission prompt yet.

On each request niminal sends a stable prefix: the built-in system prompt, then
`AGENTS.md` from `~/.niminal/` and from the git root toward the workspace
(less-specific first). Nested `AGENTS.md` files are attached when you `read` a
path under them, not tacked onto every request. Conversation turns come after
that prefix so providers can reuse the cached head of the prompt.

Assistant replies render as Markdown in the TUI: headings, lists, quotes, tables,
inline emphasis, and fenced code. Bold is yellow, italic uses the terminal italic
plus magenta, links are cyan. Drag-copy and `/copy` use the original text.

## Library

Other programs can link `niminal` and include `<niminal/agent.hpp>` for an
OpenAI-compatible streaming agent loop. Workspace tools live in the app, not
the library.
