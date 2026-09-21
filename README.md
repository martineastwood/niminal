# niminal

A local coding agent as a single native binary. You describe a change, niminal
reads and edits the workspace, and streams the turn in your terminal.

This is the C++ niminal tree: a small AI library plus the coding-agent app
(print mode and a TUI).

## Prerequisites

- CMake 3.22 or later
- a C++23 compiler
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

Set `max_steps` in that file to limit the tool loop for each user turn. Omit it
or set it to `0` for the default unbounded loop. The `--max-steps N` flag
overrides the config value for the current process, and `0` means unlimited.

Compaction tuning also lives in that file:

```json
{
  "compaction_enabled": false,
  "context_window": 200000,
  "reserve_tokens": 32768,
  "keep_recent_tokens": 40000
}
```

- `compaction_enabled` (default `true`) turns automatic compaction off.
  `/compact` and overflow recovery still work when you ask for them.
- `context_window` (default `128000`) is the token budget compaction measures
  against.
- `reserve_tokens` (default `16384`) is headroom kept for the answer;
  compaction triggers when the estimated context exceeds
  `context_window - reserve_tokens`.
- `keep_recent_tokens` (default `20000`) is the recent history kept verbatim
  while older turns are summarized.
- The summary generation itself is capped at 4096 output tokens.

Thinking traces appear as a compact preview in the TUI by default, capped at
`thinking_preview_chars` characters (default `360`) and `thinking_preview_lines`
lines (default `4`). Set `"show_thinking": true` in `~/.niminal/config.json` to
show the full streamed trace in the transcript instead.

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
- `/help`, `/provider`, `/model`, `/thinking`, `/theme`, `/permissions`, `/trust`,
  `/yolo`, `/models refresh`, `/session`, `/name`, `/resume`, `/search`, `/fork`,
  `/export`, `/delete`, `/restore`, `/new`, `/clear`, `/copy`, `/compact`,
  `/reload`, `/skill:NAME`, `/NAME`, `/quit`

- Tab completes a slash command. Up/Down moves through the list. Type `/model `
  to pick from the models.dev catalog for the active provider (filter from two
  characters). Type `/thinking ` to pick a level the current model supports.
  Type `/theme ` to pick `light`, `dark`, or `auto`. Type `/resume ` and Tab to
  pick a session.

Skills are `SKILL.md` files under `~/.niminal/skills/<name>/` or the current
workspace's `.agent/skills/<name>/`, `.agents/skills/<name>/`, or
`.niminal/skills/<name>/` (later workspace roots win on name conflicts).
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

## Extensions and hooks

Extensions are persistent programs that register tools, slash commands, and
lifecycle hooks over JSON lines. Put each extension in one of these folders:

```text
~/.agents/extensions/NAME/extension.json
~/.niminal/extensions/NAME/extension.json
<workspace>/.agents/extensions/NAME/extension.json
<workspace>/.niminal/extensions/NAME/extension.json
```

Project extensions load only after you trust the workspace. Global extensions
always load. Use `/reload` after changing a manifest or extension program.

Create `.niminal/extensions/hello/extension.json`:

```json
{
  "name": "hello",
  "command": ["./extension.py"],
  "response_timeout_seconds": 30
}
```

Then create an executable `extension.py` beside it:

```python
#!/usr/bin/env python3
import json, sys

def send(value):
    print(json.dumps(value), flush=True)

send({
    "type": "register",
    "commands": [{"name": "hello", "description": "Say hello"}],
    "tools": [{
        "name": "hello_tool",
        "description": "Return a greeting.",
        "input_schema": {"type": "object"},
        "capabilities": ["read"]
    }],
    "events": ["tool_call", "tool_result"]
})

for line in sys.stdin:
    message = json.loads(line)
    if message["type"] == "shutdown":
        break
    if message["type"] == "command":
        send({"type": "response", "id": message["id"],
              "message": "Hello " + message["arguments"]})
    elif message["type"] == "tool":
        send({"type": "response", "id": message["id"],
              "content": [{"type": "text", "text": "Hello"}]})
    elif message["type"] == "event":
        send({"type": "response", "id": message["id"]})
```

The host sends `initialize` first. Your program replies with one `register`
object, then answers every `command`, `tool`, and `event` request with a
`response` carrying the same `id`. Stdout is reserved for protocol messages.

Extensions can subscribe to `tool_call`, `tool_result`, `turn_start`,
`turn_end`, `context`, `session_start`, `session_end`,
`session_before_compact`, and `session_compact`. A `tool_call` hook can return
`allow: false` or replacement `arguments`. A `tool_result` hook can replace
`output` and `is_error`. Context hooks can append `system` strings and
`messages`. Compaction hooks can block compaction, add an `instruction`, or
provide a complete `compaction` result.

Responses and unsolicited `update` messages can set footer `status`, display a
`widget` or `notification`, persist an `entry` in the session, and queue a
`user_message`. Extension tools use the normal permission prompt. Tools whose
capabilities contain only `read` and `user` are treated as read-only.

The current C++ host accepts text tool-result parts. UI and host requests are
answered as unavailable, and image result parts are not added to the model
context yet.

### External tools

External tools are short-lived executables described by `tool.json`. niminal
starts one only when the model calls it, sends the tool arguments as one JSON
object on stdin, and expects one JSON value on stdout.

Put a tool in `~/.niminal/tools/NAME/`, `.agent/tools/NAME/`,
`.agents/tools/NAME/`, or `.niminal/tools/NAME/`:

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

`name`, `description`, `command`, and `input_schema` are required.
`command` must be a non-empty string array. The first item is resolved
relative to the manifest directory and later items are fixed arguments. The
process runs in the workspace. `timeout_seconds` defaults to 30 and output is
capped at 100,000 bytes. The executable must be executable on POSIX systems.

Supported capabilities are `read`, `write`, `shell`, `network`, and `user`.
Tools that declare only `read` are treated as read-only. Project tools are
loaded only for trusted workspaces. Use `/reload` after changing a manifest.
Invalid manifests and names that collide with built-in tools are skipped with a
warning.

`/model ID`, `/provider NAME`, and `/thinking LEVEL` are saved to
`~/.niminal/config.json`. The thinking value is a shared ladder
(`none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`). When you switch
models, niminal keeps that saved level and maps it to whatever the new model
actually accepts, using models.dev `reasoning` / `reasoning_options` plus
Anthropic's adaptive effort names. `/thinking` with no argument prints the
mapped level. Unset thinking leaves the provider default.

The TUI ships one dark and one light palette. `/theme light`, `/theme dark`, and
`/theme auto` save `theme` to `~/.niminal/config.json`, and `/theme` with no
argument prints the mode plus the palette auto resolved to. `auto` (the default)
asks the terminal for its background color with an OSC 11 query at startup, falls
back to `COLORFGBG`, and assumes dark when neither answers. Set `"theme"` in the
config file to pick a palette without touching the TUI.

On startup, niminal loads the config file, then applies `NIMINAL_MODEL` /
`NIMINAL_API_URL` / `NIMINAL_THINKING` if they are set, then `--provider`,
`--model`, and `--thinking`.

Resume the latest session for this directory with `--resume`, or a specific
file with `--session ID`. `/resume` lists the newest 20 sessions that belong to
the current workspace. `/resume ID` loads one, including sessions started
somewhere else (those show a workspace warning). `/new` starts a fresh file;
the old one stays on disk. `/clear` does the same. `--no-session` keeps the
transcript in memory only, and cannot be combined with `--resume` or
`--session`. `/search TEXT` matches text across this workspace's sessions.

`/fork [title]` copies the current session into a new file (recorded as its
parent) and switches to it. `/export [PATH]` writes the session as Markdown, or
as JSON when the path ends in `.json`; the default is `<id>.md` in the
workspace. `/delete ID` moves a session to `~/.niminal/sessions/.trash`, and
`/restore` lists what is there while `/restore ID` brings one back.

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

JSON mode emits the same turn as versioned JSONL events, which is useful when
another program drives niminal:

```sh
/path/to/niminal/build/niminal --mode json "Explain what this repo does."
cat README.md | /path/to/niminal/build/niminal --mode json "Summarize this"
```

The stream starts and ends with `session_start` and `session_end`, and includes
the user message, streamed assistant deltas, tool calls and results, step
boundaries, and the final assistant message. Diagnostics stay on stderr.

RPC mode keeps the process running and accepts one JSON command per stdin line:

```sh
/path/to/niminal/build/niminal --mode rpc
```

For example, inspect the session and then shut it down:

```sh
printf '%s\n' \
  '{"id":"1","type":"get_state"}' \
  '{"id":"2","type":"shutdown"}' \
  | /path/to/niminal/build/niminal --mode rpc
```

Start a turn with `prompt`, queue `steer` or `follow_up` messages while it is
busy, inspect queues with `get_state`, and stop it with `interrupt` or
`shutdown`. Every response and event is versioned JSONL. Queue delivery modes
are `all` and `one-at-a-time`, and are saved in `~/.niminal/config.json` as
`steering_mode` and `follow_up_mode`.

Optional environment:

- `NIMINAL_MODEL` (overrides `~/.niminal/config.json`)
- `NIMINAL_API_URL` (overrides the config file, default OpenRouter chat completions)
- `NIMINAL_THINKING` (overrides `thinking` in the config file)

Optional flags: `--model ID`, `--provider NAME`, `--thinking LEVEL`, `--mode json|rpc`,
`--api-key KEY`, `--max-steps N`, `--resume`, `--session ID`, `--no-session`, `--yolo`,
`--approve`, `--no-approve`.

File tools stay inside the current directory. `grep` and `glob` use git's
tracked and untracked files and honor `.gitignore`, so `build/` stays out of
search. In the TUI, `read`, `grep`, `glob`, `edit`, `write`, and `skill` run
without approval. Shell commands and other tools ask before they run. Press
Enter for once, `s` for the session, `p` to save a project grant, or `n` to
deny. Commands such as `rm`, `sudo`, `curl`, `ssh`, and `git reset` are always
asked again and cannot be remembered.

Project prompts, skills, extensions, and `.niminal/permissions.json` are optional
local customizations. The first interactive launch in a workspace that contains them
asks whether to load them. The answer is saved in `~/.niminal/trust.json`.
Use `/trust on` or `/trust off` to change it later, or use `--approve` and
`--no-approve` for one process. `/permissions` lists grants and
`/permissions clear` removes project grants. `/yolo` and `--yolo` skip approval
prompts for the current process only.

Print, JSON, and RPC modes have no approval UI, so tools run without prompting.
Use them only with workspaces you trust. Approval is not a sandbox: shell
commands still run as your user, with your environment.

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
