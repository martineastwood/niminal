# niminal

A native coding agent for your repository. Point it at a project, describe the
change you want, and niminal inspects the workspace, edits files, runs commands
you approve, and keeps the session so you can come back later.

Fast startup, low memory, and no idle CPU until you ask.

**Documentation:** [niminal.dev](https://niminal.dev)

## Install

macOS and Linux (x86_64; macOS also supports arm64):

```sh
curl -fsSL https://niminal.dev/install.sh | sh
```

The installer puts the `niminal` binary in `~/.local/bin`. Add that directory
to your `PATH` if it is not already there.

Pin a release with `NIMINAL_VERSION=v0.1.0`, or override the install location
with `NIMINAL_INSTALL_DIR`. See [Install](https://niminal.dev/guides/install/)
for build-from-source instructions.

## Quickstart

Set a provider key. OpenRouter is the default:

```sh
export OPENROUTER_API_KEY=your-key
cd /path/to/your/project
niminal
```

Try a task:

```text
Explain how this project runs its tests, then suggest the smallest useful fix
for the failing parser test.
```

The interactive footer shows the active provider, model, thinking level, token
totals, and estimated session cost. File reads, searches, and workspace edits are
built in. Shell commands normally ask for approval the first time.

### One-shot and piped input

Pass a prompt for a turn that exits when it finishes:

```sh
niminal fix the failing parser test
cat README.md | niminal summarize this
```

Resume a saved session:

```sh
niminal --resume
niminal --session 1789233281025102
```

See the [Quickstart](https://niminal.dev/guides/quickstart/) for provider
options, flags, and project trust.

## Providers

Switch provider with `--provider NAME` or `/provider NAME`, and export the
matching key:

| Provider | Credential |
| --- | --- |
| OpenRouter (default) | `OPENROUTER_API_KEY` |
| Anthropic | `ANTHROPIC_API_KEY` |
| Google Gemini | `GEMINI_API_KEY`, `GOOGLE_API_KEY`, or `GOOGLE_GENERATIVE_AI_API_KEY` |
| OpenAI | `OPENAI_API_KEY` |
| Mistral | `MISTRAL_API_KEY` |
| Hyper | `HYPER_API_KEY` |
| OpenCode Go or Zen | `OPENCODE_API_KEY` |

For a custom environment variable, use `~/.niminal/auth.json`:

```json
{"openai": {"key": "$MY_OPENAI_KEY"}}
```

Keys resolve in this order: `--api-key`, `auth.json`, then the standard
provider environment variable. Keep `auth.json` private with `chmod 600`.

Use `/model`, `/thinking`, and `/theme` in the TUI, or pass `--model`,
`--thinking`, and related flags for one run. Defaults and overrides live in
`~/.niminal/config.json`. See
[Models and providers](https://niminal.dev/guides/models-and-providers/) and
[Configuration](https://niminal.dev/guides/configuration/).

## Working in the terminal

With no prompt, niminal opens a fullscreen TUI. Each conversation is a session
saved under `~/.niminal/sessions`.

- Enter sends. While a turn runs, Enter queues a steering message for the next
  model request.
- Esc interrupts a running turn, sends queued messages now, or clears the
  composer when idle.
- Ctrl-V attaches a PNG, JPEG, or WebP screenshot. You can also `@mention` an
  image or drop its path into the composer.
- Tab completes slash commands. Type `/help` for the full list.
- `!command` runs a shell command and includes its output in the next model turn.
  `!!command` runs without sending output to the model.
- `/resume`, `/new`, `/fork`, and `/export` manage sessions.
- `/trust on` loads project skills, prompts, extensions, and permissions after
  the first trust prompt.

Keybindings, queues, mentions, and approval prompts are covered in the
[Interactive TUI](https://niminal.dev/guides/interactive-tui/) guide.

## Extend niminal

Add behavior without forking the agent:

- **Skills** (`SKILL.md` files) load procedures when a task needs them.
- **Prompt templates** turn recurring requests into slash commands.
- **External tools** expose a one-shot executable as a typed model tool.
- **Extensions** are long-running programs that add tools, commands, hooks, and
  live status over JSON lines. Any language works.

Put project files under `.niminal/` or the portable `.agents/` layout. Global
copies live under `~/.niminal/` and `~/.agents/`.

- [Skills](https://niminal.dev/guides/skills/)
- [Prompt templates](https://niminal.dev/guides/prompt-templates/)
- [External tools](https://niminal.dev/guides/external-tools/)
- [Extensions and hooks](https://niminal.dev/guides/extensions-and-hooks/)
- [Instructions](https://niminal.dev/guides/instructions/) (`AGENTS.md` rules)

## Scripting and automation

The same agent runs in several modes:

| Mode | Use when |
| --- | --- |
| TUI (default) | Interactive work in the terminal |
| Print | One turn on stdout, then exit |
| `--mode json` | Versioned JSONL events for a single run |
| `--mode rpc` | A long-running process driven by stdin commands |

Print, JSON, and RPC modes have no approval UI, so tools run without
prompting. Use them only with workspaces you trust, and narrow tools when you
can:

```sh
niminal "summarize the README" --tools read
niminal --mode json "Explain what this repo does."
```

See [JSON mode](https://niminal.dev/reference/json-mode/) and
[RPC mode](https://niminal.dev/reference/rpc-mode/).

## Security

Approval is not a sandbox. Shell commands run as your user, with your
environment. Reads, searches, and workspace edits run freely in the TUI; shell
commands and extension tools ask before they run unless you use `/yolo` or
`--yolo`.

See [Security](https://niminal.dev/guides/security/) and
[Permissions](https://niminal.dev/guides/permissions/).

## Documentation

Full guides and reference material live at [niminal.dev](https://niminal.dev):

- [Install](https://niminal.dev/guides/install/)
- [Quickstart](https://niminal.dev/guides/quickstart/)
- [Sessions](https://niminal.dev/guides/sessions/)
- [Commands and shortcuts](https://niminal.dev/reference/commands/)
- [Keybindings](https://niminal.dev/reference/keybindings/)

## Build from source

For contributors and packagers:

```sh
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build
```

The binary is `build/niminal`. Run `./dev check` before landing changes. The
`niminal::ai` library target is documented in
[Architecture](https://niminal.dev/reference/architecture/).
