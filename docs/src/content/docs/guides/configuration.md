---
title: Configuration
description: Global settings, provider defaults, compaction, and environment overrides.
---

niminal reads one optional configuration file:

| File | Applies to |
| --- | --- |
| `~/.niminal/config.json` | Everything you run |
| `~/.niminal/auth.json` | Provider credentials |
| `~/.niminal/keybindings.json` | Interactive TUI shortcuts |

There is no config command to run first: a missing file is normal, and every
setting has a default. Many people never write more than a provider and a model.
In the interactive TUI, run `/settings` to edit these values in an overlay
without opening the file by hand. You can [change TUI shortcuts](/reference/keybindings/) separately in
`keybindings.json`; changes take effect when you next start niminal.

Credentials are not stored in `config.json`. Export the matching provider
environment variable, add a credential to `auth.json`, or pass `--api-key KEY`
for one process. Keys are resolved in that order from highest to lowest
priority: `--api-key`, `auth.json`, then the provider's standard environment
variable.

An auth entry can contain a literal key or an environment-variable reference:

```json title="~/.niminal/auth.json"
{
  "openai": {"key": "$MY_OPENAI_KEY"},
  "anthropic": {"key": "sk-ant-your-key"}
}
```

Both `$MY_OPENAI_KEY` and `${MY_OPENAI_KEY}` are supported. The auth file is
not created by niminal, so set its permissions to user-only when it contains
literal keys:

```sh
chmod 600 ~/.niminal/auth.json
```

## A minimal config

```json title="~/.niminal/config.json"
{
  "provider": "anthropic",
  "model": "claude-sonnet-4-6"
}
```

## Settings

| Key | Default | Meaning |
| --- | --- | --- |
| `provider` | `openrouter` | Active provider name |
| `model` | provider default | Active model id |
| `api_url` | provider endpoint | Override the API base URL |
| `thinking` | unset | Reasoning level: `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `max` |
| `show_thinking` | `false` | Start new thinking cards expanded in the TUI |
| `theme` | `auto` | `light`, `dark`, `auto`, or a name from `~/.niminal/themes/` |
| `editor` | unset | External editor command for Ctrl-G. Overrides `$VISUAL` and `$EDITOR` |
| `steering_mode` | `one-at-a-time` | RPC queue delivery: `all` or `one-at-a-time` (does not change TUI steering) |
| `follow_up_mode` | `one-at-a-time` | Same for follow-up queue in RPC mode |
| `max_steps` | `0` (unlimited) | Tool loop cap per user turn |
| `compaction_enabled` | `true` | Automatic compaction before large requests |
| `reserve_tokens` | `16384` | Headroom kept when deciding whether to compact |
| `keep_recent_tokens` | `20000` | Recent history kept verbatim when compacting |
| `context_window` | `128000` (implicit) | Token budget compaction measures against |
| `providers.<name>.last_model` | per provider | Restored when you switch back to that provider |

When `thinking` is unset, the provider default applies. `/thinking` with no
argument prints the mapped level for the current model.

Set `editor` to a shell command such as `hx` or `vim` when you want a niminal
editor that is not `$VISUAL` or `$EDITOR`. If `editor` is unset, niminal uses
`$VISUAL`, then `$EDITOR`, then `nano`.

## Compaction example

```json title="~/.niminal/config.json"
{
  "compaction_enabled": false,
  "context_window": 200000,
  "reserve_tokens": 32768,
  "keep_recent_tokens": 40000
}
```

- `compaction_enabled: false` turns automatic compaction off. `/compact` and
  overflow recovery still work when you ask for them.
- `context_window` is the token budget compaction measures against.
- `reserve_tokens` is headroom kept for the answer. Compaction triggers when
  the estimated context exceeds `context_window - reserve_tokens`.
- `keep_recent_tokens` is the recent history kept verbatim while older turns
  are summarized.

## Startup overrides

Startup flags and environment variables override the config for one process only
and are never written back:

| Source | Effect |
| --- | --- |
| `NIMINAL_MODEL` | Overrides `model` |
| `NIMINAL_API_URL` | Overrides `api_url` |
| `NIMINAL_THINKING` | Overrides `thinking` |
| `--provider`, `--model`, `--thinking`, `--api-key`, `--tools`, `--max-steps` | Same as their names suggest |
| `--approve`, `--no-approve` | Choose whether project-local resources load |
| `--system-prompt`, `--append-system-prompt` | Replace or extend the system prompt |
| `--no-context-files`, `-nc` | Skip `AGENTS.md` and `CLAUDE.md` discovery |
| `--version`, `--help` | Print the version or usage and exit |

Load order: config file, then environment, then CLI flags. Provider credentials
are resolved separately as `--api-key`, `auth.json`, then the provider
environment variable.

## What niminal writes

These commands save settings to `~/.niminal/config.json` immediately:

| Command | Keys written |
| --- | --- |
| `/settings` | Any key you change in the overlay |
| `/provider` | `provider`, `model`, and `providers.<name>.last_model` |
| `/model` | `model`, and `providers.<active>.last_model` |
| `/thinking` | `thinking` (use `/settings` to clear it) |
| `/theme` | `theme` |
| RPC `set_steering_mode` | `steering_mode` |
| RPC `set_follow_up_mode` | `follow_up_mode` |

## Where to go next

- [Models and providers](/guides/models-and-providers/) for switching, thinking
  levels, and the models.dev catalog
- [Context and compaction](/guides/context-and-compaction/) for automatic summarization
- [Files and directories](/reference/files-and-directories/) for every file niminal
  reads and writes
- [Security](/guides/security/) for how trust gates project resources
