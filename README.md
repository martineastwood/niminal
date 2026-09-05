# niminal

A minimal native coding agent written in Nim.

## Build and test

```sh
nimble build
nimble test
```

niminal depends on the sibling [nimgent](../nimgent) package (LLM client
library). Local development resolves it via `--path:"../nimgent/src"` in
`nim.cfg`. With Docker Compose, `../nimgent` is mounted at `/nimgent`.

After nimgent is published to the Nimble directory, uncomment
`requires "nimgent >= 0.1.0"` in `niminal.nimble` (the path still wins when
the sibling checkout is present).

Windows is not supported natively. Use [WSL](https://learn.microsoft.com/windows/wsl) and build inside the Linux environment.

Set the provider's API key before starting the agent:
`OPENROUTER_API_KEY` for OpenRouter, `OPENAI_API_KEY` for OpenAI, or
`ANTHROPIC_API_KEY` for Anthropic.
Optional project configuration is read from `.niminal/config.json` in the
workspace and overlays `~/.niminal/config.json`. Global files live in
`~/.niminal/`:

- `~/.niminal/config.json` — default provider and model
- `~/.niminal/AGENTS.md` — personal instructions (all projects)
- `~/.niminal/skills/` — global skills
- `~/.niminal/tools/` — global external tools
- `~/.niminal/hooks/` — global lifecycle hooks
- `~/.niminal/sessions/` — saved sessions
- `~/.niminal/models-dev.json` — cached model metadata

`/model` and `/thinking` write immediately: to the project file if it exists,
otherwise to the global file (created if needed). `/model` sets
`default_provider` and `default_model`; `/thinking` sets `agent.thinking`.

Configuration example:

```json
{
  "default_provider": "openrouter",
  "default_model": "deepseek/deepseek-v4-flash-0731",
  "providers": {
    "openrouter": {
      "api_key_env": "OPENROUTER_API_KEY"
    },
    "openai": {
      "api_key_env": "OPENAI_API_KEY"
    }
  },
  "agent": {
    "max_tokens": 4096,
    "request_timeout": 300
  }
}
```

Run `./niminal` from the workspace you want the agent to modify.
Use `/session` to print the current session ID and `/resume` to list or
resume sessions. `/resume` lists this workspace (newest 20), with recency
and the first user message; `/resume ID` restores that transcript and
the last model used. A session from another project still loads by ID,
with a warning. `/model name` switches the model for later turns.
`./niminal --resume` continues the latest session for this workspace.
A specific session can be selected with `./niminal --session ID`.

Project instructions are loaded from `~/.niminal/AGENTS.md`, then from
`AGENTS.md` files between the repository root and the workspace. Passive
skills can be placed in `.niminal/skills/<name>/SKILL.md`,
`.agent/skills/<name>/SKILL.md`, or `~/.niminal/skills/<name>/SKILL.md`;
their metadata is advertised to the model and full bodies are loaded
only through the `read_skill` tool. Type `/<skill>` (optionally followed by
a request) to load a skill into the next turn. Built-in commands win when
names collide.

External tools are discovered the same way under `tools/` instead of
`skills/`: `~/.niminal/tools/`, `<workspace>/.agent/tools/`, then
`<workspace>/.niminal/tools/` (later roots override the same name). Each
child directory needs a `tool.json` and an executable; the agent reads
manifests at startup and only spawns the process when the model calls the
tool. Built-in tool names always win over extensions.

Lifecycle hooks use the same discovery layout under `hooks/` with a
`hook.json` per child directory. Supported events: `pre_tool_call`,
`post_tool_call`, `session_start`, `session_end`. Hooks are ephemeral
JSON processes (stdin in, JSON out). Failures are fail-open (warn and
continue); only an explicit `{"allow": false}` from `pre_tool_call`
blocks a tool. Later roots override the same hook `name`. Opening
niminal fires `session_start`; `/new` and `/resume` fire `session_end`
then `session_start`; clean exit fires `session_end`.
