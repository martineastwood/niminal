---
title: Models and providers
description: Switch providers, pick models, map thinking levels, and refresh the catalog.
---

niminal keeps one request shape (OpenAI-style messages and tools) and hands each
model step to CAIL, which talks to the provider on the wire.

## Wired providers

| Provider | Default model | Key environment variables | Endpoint |
| --- | --- | --- | --- |
| `openrouter` | `openai/gpt-4o-mini` | `OPENROUTER_API_KEY` | `https://openrouter.ai/api/v1/chat/completions` |
| `anthropic` | `claude-sonnet-4-6` | `ANTHROPIC_API_KEY` | `https://api.anthropic.com/v1` |
| `google` | `gemini-3.5-flash-lite` | `GEMINI_API_KEY`, `GOOGLE_API_KEY`, or `GOOGLE_GENERATIVE_AI_API_KEY` | `https://generativelanguage.googleapis.com/v1beta` |
| `foundry` | first Foundry entry in `models.json` | `AZURE_FOUNDRY_API_KEY` | configured per model |
| `openai` | `gpt-5` | `OPENAI_API_KEY` | `https://api.openai.com/v1` |
| `local` | first local entry in `models.json` | optional `auth.json` key | configured per model |
| `ollama` | `gemma4:31b` | `OLLAMA_API_KEY` | `https://ollama.com/v1/chat/completions` |
| `hyper` | `deepseek-v4-flash` | `HYPER_API_KEY` | `https://hyper.charm.land/v1/chat/completions` |
| `mistral` | `mistral-vibe-cli-with-tools` | `MISTRAL_API_KEY` | `https://api.mistral.ai/v1/chat/completions` |
| `opencode` | `deepseek-v4.1-flash` | `OPENCODE_API_KEY` | `https://opencode.ai/zen/go/v1` |
| `opencodezen` | `deepseek-v4-flash` | `OPENCODE_API_KEY` | `https://opencode.ai/zen/v1` |

OpenRouter is the default at startup. Selecting `codex` errors: Codex App Server
is not wired.

Switch providers with `/provider NAME` or `--provider NAME`. Each provider
remembers its last model in `providers.<name>.last_model` inside
`~/.niminal/config.json`.

`/provider` alone prints the active name, model, and your endpoint override,
or `(provider default)` when no override is set. Credentials can come from the
environment variables above, `~/.niminal/auth.json`, or `--api-key`.

## Use Microsoft Foundry

Add your Foundry deployments to `~/.niminal/models.json` and export your key:

```sh
export AZURE_FOUNDRY_API_KEY=your-key
```

```json title="~/.niminal/models.json"
{
  "models": [
    {
      "provider": "foundry",
      "name": "coding",
      "model": "my-coding-deployment",
      "api_url": "https://<resource>.cognitiveservices.azure.com/openai/responses?api-version=2025-04-01-preview"
    }
  ]
}
```

Run `niminal --provider foundry`, then use `/model coding`. Add one entry per
deployment you want to select. `name` is the choice shown by niminal; `model`
must match your Foundry deployment name. Use the Responses endpoint URL and API
version shown for that deployment. You can optionally set `context_window` for
compaction.
You can also store the key as `"foundry": {"key": "$AZURE_FOUNDRY_API_KEY"}`
in `~/.niminal/auth.json`.

Ollama Cloud supports streaming and tool calls through its OpenAI-compatible
chat API. You can choose another available Ollama Cloud model with
`--model ID` or `/model ID`.

## Use a local model

You can connect niminal to models served by llama.cpp or Ollama. For llama.cpp,
start `llama-server` with a GGUF model and a chat template that supports tool
calls:

```sh
llama-server -m /path/to/model.gguf --alias coding --ctx-size 32768 --jinja
```

Create `~/.niminal/models.json` with the same model alias and context size:

```json title="~/.niminal/models.json"
{
  "models": [
    {
      "provider": "local",
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
`/models` to see your local entries and `/model coding` to select one. The
`name` is the choice shown in niminal; `model` is sent to the server. Each entry
can point to a different running server. Local servers do not require an API
key by default. If you protect yours with a key, set `local` in
`~/.niminal/auth.json`.

To use Ollama, run a model and copy its name from `ollama list`:

```sh
ollama run hf.co/Qwen/Qwen3-1.7B-GGUF:Q8_0
ollama list
```

Add another entry to the `models` array with `"runtime": "ollama"`, the name
reported by `ollama list` as `model`, and
`"api_url": "http://127.0.0.1:11434/v1/chat/completions"`. Set
`context_window` to the context size you configured in Ollama. Then use
`/model NAME` to select it. The TUI footer shows the provider, runtime, and model,
for example `local/ollama/qwen3-1.7b`.

The `context_window` should match `--ctx-size`; niminal uses it to decide when
to compact the session. Tool calls also need a model with a suitable chat
template. The `runtime` field accepts `llamacpp` and `ollama`. Local and Foundry
entries share the same `models` array. A name can be used once per provider.

## Model selection

- `/model` alone prints the current model
- `/model ID` switches for later turns and saves the choice
- With the local or Foundry provider, `ID` is a name from `models.json`. With
  other hosted providers, `ID` is sent to that provider.
- `/model ` plus Tab suggests configured local or Foundry models, or hosted
  catalog matches for other providers

Use `/models` to list entries from `models.json` when the local or Foundry
provider is active.

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
