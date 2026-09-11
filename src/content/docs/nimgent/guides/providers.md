---
title: Providers
description: One API surface for every provider — and where provider specifics live.
---

nimgent talks to several LLM providers through one shape. You import an
adapter, create the provider with your API key, and bind a model. From there,
every function in the library — generation, streaming, tools, structured
output — works identically no matter which adapter is underneath.

That's the point of the design: **the adapter is a translation layer**. It owns
authentication, request encoding, streaming, and response normalization, so
your application code deals with models, not HTTP payloads. Switching
providers is a two-line change, and the rest of your code doesn't notice.

## Supported adapters

```nim
import nimgent/providers/[anthropic, google, hyper, openai, openrouter]
```

| Adapter | Constructor | API surface |
| --- | --- | --- |
| OpenAI | `openAI(apiKey)` | Responses API and Chat Completions compatibility |
| Anthropic | `anthropic(apiKey)` | Native Messages API and streaming |
| Google | `google(apiKey)` | Native Gemini generation, hosted tools, and embeddings |
| OpenRouter | `openRouter(apiKey)` | OpenAI-compatible routing and models |
| Hyper | `hyper(apiKey)` | OpenAI-compatible Chat Completions endpoint |

```nim
let model = openAI(getEnv("OPENAI_API_KEY")).model("gpt-4o-mini")
let response = generateText(model, prompt = "Explain this function.")
```

A `LanguageModel` is just a provider plus a model id — it's a value, so you can
store it, pass it around, and create several for the same provider.

## Two layers of settings

Provider-specific settings come in two forms, and knowing which to reach for
solves most configuration questions:

- **Typed options** (`providerOptions`) — settings nimgent models with Nim
  types. Compile-time checked, discoverable in your editor.
- **Raw options** (`options`) — a `JsonNode` escape hatch using the provider's
  native field names. Anything nimgent doesn't model yet goes here.

Both end up in the same request; if a field is set in both, raw `options` are
applied first and typed options win.

### Typed options

Each adapter has a namespace on `ProviderOptions`:

```nim
import std/options
import nimgent
import nimgent/providers/openai

let response = generateText(
  openAI(getEnv("OPENAI_API_KEY")).model("gpt-4o-mini"),
  prompt = "Explain this code.",
  providerOptions = ProviderOptions(
    openai: OpenAIOptions(
      reasoningEffort: some("high"),
      store: some(false))))
```

Only the namespace matching the provider is applied — an `OpenAIOptions` on a
Google model is ignored. Every field is an `Option[T]` with a deliberate rule:

- **Unset (`none`)** fields are omitted from the request entirely.
- **Set fields are sent exactly**, including explicit `false`, `0`, and empty
  sequences. Setting them matters; nimgent won't second-guess you.

Typed options also validate combinations before the request: Anthropic's
`budgetTokens`, for example, requires `thinking = some(EnabledThinking)` and a
budget of at least 1024 — you get that error locally, not as a 400 from the
API.

The namespaces mirror each adapter's real capabilities:

| Namespace | Modeled settings |
| --- | --- |
| `OpenAIOptions` | `reasoningEffort`, `parallelToolCalls`, `store`, `user`, `dimensions` (embeddings) |
| `AnthropicOptions` | `thinking`, `budgetTokens`, `effort` |
| `GoogleOptions` | `reasoningEffort` |
| `OpenRouterOptions` | `routing` (model `order`, `only`, `ignore`) |

### Raw options

For anything not modeled — a brand-new API field, a niche flag, a Hyper-only
setting — pass JSON with the provider's native field names:

```nim
let response = generateText(
  model,
  prompt = "Be concise.",
  options = %*{"temperature": 0.2})
```

Options must be a JSON object. Your JSON is copied before the adapter touches
it, so provider encoding never mutates application-owned values. Raw options
are also the only route for the `hyper` namespace, which has no typed form —
or set `ProviderOptions(extra = %*{"hyper": {...}})` to keep it beside the
other typed namespaces.

## Capabilities

Applications that pick providers at runtime — a settings screen, a fallback
chain — can ask what a model's provider supports instead of hard-coding it:

```nim
if model.provider.supports(pcStructuredOutput):
  # safe to use generateObject with omNative
  discard

if model.provider.supports(pcHostedTools):
  tools.add hostedTool("web_search")
```

| Capability | Meaning |
| --- | --- |
| `pcStreaming` | Token streaming via `streamText` |
| `pcTools` | Local tool definitions |
| `pcStructuredOutput` | Native structured output (`generateObject`) |
| `pcImages` / `pcFiles` | Image or file inputs |
| `pcHostedTools` | Provider-executed tools like `web_search` |
| `pcEmbeddings` | `embed` / `embedMany` |

## Which adapter should I pick?

All of them speak the same protocol, so the honest answer is: the one matching
the key you have. Practical guidance:

- **OpenAI** and **Anthropic** when you want first-class support for that
  vendor's newest features (native structured output, thinking, hosted tools).
- **Google** for Gemini, including hosted tools and embeddings.
- **OpenRouter** to reach many models — including non-OpenAI ones — through
  one key, with routing controls.
- **Hyper** for an OpenAI-compatible self-hosted or alternative endpoint.

Since the surface is shared, testing against OpenRouter and shipping on OpenAI
(and vice versa) is a supported pattern, not a migration.
