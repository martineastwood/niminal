---
title: Models and providers
description: Switch providers, pick models, map thinking levels, and refresh the catalog.
---

niminal speaks one request shape (OpenAI-style messages and tools) and translates
that to each provider's native API before it goes on the wire.

## Wired providers

| Provider | Default model | Key environment variables | Endpoint |
| --- | --- | --- | --- |
| `openrouter` | `openai/gpt-4o-mini` | `OPENROUTER_API_KEY` | `https://openrouter.ai/api/v1/chat/completions` |
| `anthropic` | `claude-sonnet-4-6` | `ANTHROPIC_API_KEY` | `https://api.anthropic.com/v1/messages` |
| `google` | `gemini-3.5-flash-lite` | `GEMINI_API_KEY`, `GOOGLE_API_KEY`, or `GOOGLE_GENERATIVE_AI_API_KEY` | `https://generativelanguage.googleapis.com/v1beta` |
| `openai` | `gpt-5` | `OPENAI_API_KEY` | `https://api.openai.com/v1/chat/completions` |
| `local` | first entry in `models.json` | optional `auth.json` key | configured per model |
| `ollama` | `gemma4:31b` | `OLLAMA_API_KEY` | `https://ollama.com/v1/chat/completions` |
| `hyper` | `deepseek-v4-flash` | `HYPER_API_KEY` | `https://hyper.charm.land/v1/chat/completions` |
| `mistral` | `mistral-vibe-cli-with-tools` | `MISTRAL_API_KEY` | `https://api.mistral.ai/v1/chat/completions` |
| `opencode` | `deepseek-v4.1-flash` | `OPENCODE_API_KEY` | `https://opencode.ai/zen/go/v1/chat/completions` |
| `opencodezen` | `deepseek-v4-flash` | `OPENCODE_API_KEY` | `https://opencode.ai/zen/v1/chat/completions` |

OpenRouter is the default at startup. Selecting `codex` errors: Codex App Server
is not wired.

Switch providers with `/provider NAME` or `--provider NAME`. Each provider
remembers its last model in `providers.<name>.last_model` inside
`~/.niminal/config.json`.

`/provider` alone prints the active name, model, endpoint, and the provider's
standard environment variable. Credentials can also come from
`~/.niminal/auth.json` or `--api-key`.

Ollama Cloud supports streaming and tool calls through its OpenAI-compatible
chat API. You can choose another available Ollama Cloud model with
`--model ID` or `/model ID`.

## Use a local model

You can connect niminal to a model served by llama.cpp. Start `llama-server`
with a GGUF model and a chat template that supports tool calls:

```sh
llama-server -m /path/to/model.gguf --alias coding --ctx-size 32768 --jinja
```

Create `~/.niminal/models.json` with the same model alias and context size:

```json title="~/.niminal/models.json"
{
  "models": [
    {
      "name": "coding",
      "runtime": "llamacpp",
      "model": "coding",
      "api_url": "http://127.0.0.1:8080/v1/chat/completions",
      "context_window": 32768
    }
  ]
}
```

Run `niminal --provider local`, or use `/provider local` in the TUI. Type
`/models` to see your local entries and `/models coding` to select one. The
`name` is the choice shown in niminal; `model` is sent to llama.cpp. Each entry
can point to a different running server. Local llama.cpp servers do not require
an API key. If you protect yours with a key, set `local` in
`~/.niminal/auth.json`.

The `context_window` should match `--ctx-size`; niminal uses it to decide when
to compact the session. Tool calls also need a model with a suitable chat
template. The `runtime` field currently accepts `llamacpp`.

## Model selection

- `/model` alone prints the current id
- `/model ID` switches for later turns and saves the choice
- `/model ` plus Tab opens catalog suggestions filtered from two characters

For the `local` provider, use `/models` to list and select entries from
`models.json`. `/model` is for hosted providers.

The catalog comes from [models.dev](https://models.dev/api.json), cached at
`~/.niminal/models-dev.json`. Use `/models refresh` to fetch a fresh copy.
niminal may also refresh stale cache data in the background at TUI startup.

## Thinking levels

Shared ladder: `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`.

When you switch models, niminal keeps your saved thinking level and maps it to
whatever the new model accepts, using models.dev reasoning metadata plus
provider-specific rules.

`/thinking` with no argument prints the mapped level. Unset thinking leaves the
provider default.

Set thinking in the config file, with `NIMINAL_THINKING`, or with `--thinking`.

## Retries

Transient model connection failures retry automatically up to three times with
exponential backoff. Use `/retry` after those attempts are exhausted.

## Next steps

- [Configuration](/guides/configuration/) for saved defaults
- [Commands and shortcuts](/reference/commands/) for `/model`, `/provider`, and `/thinking`
