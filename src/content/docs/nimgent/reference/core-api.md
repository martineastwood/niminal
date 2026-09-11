---
title: Core API
description: The small set of types and entry points used by most applications.
---

## Models

| Type | Purpose |
| --- | --- |
| `Provider` | Provider adapter and capability set |
| `LanguageModel` | A text-generation model bound to a provider |
| `EmbeddingModel` | An embedding model bound to a provider |
| `ProviderRequest` | Provider-neutral request for direct adapter use |
| `ProviderResponse` | Normalized model response, usage, and steps |

Create models through a provider:

```nim
let chat = openAI(apiKey).model("gpt-4o-mini")
let embeddings = openAI(apiKey).embeddingModel("text-embedding-3-small")
```

## Generation

```nim
generateText(model, prompt = "Hello")
generateTextAsync(model, prompt = "Hello")
streamText(model, prompt = "Hello", onEvent = callback)
streamTextAsync(model, prompt = "Hello", onEvent = callback)
```

The response exposes `text`, `content`, `usage`, `finishReason`, `requestId`,
and `steps`. A multi-turn tool run aggregates usage in `totalUsage`.

## Embeddings

```nim
let one = embed(embeddings, "sunny day")
let many = embedMany(embeddings, @["sunny day", "rainy day"])
let similarity = cosineSimilarity(many.embeddings[0], many.embeddings[1])
```

## Cancellation and retries

Pass `abort` to stop before the next provider attempt or tool call:

```nim
let response = await generateTextAsync(
  model,
  prompt = "Continue until done.",
  abort = proc (): bool = shouldStop())
```

Transient HTTP and transport failures are retried by default. Configure the
limit with `maxRetries`. Context overflow, ordinary client errors, and a
started stream are not retried.

## Direct provider requests

Use a ready-made `ProviderRequest` when the application owns the request
shape. `generateText(provider, request)` and `streamText(provider, request,`
`callback)` provide retrying wrappers without running a local tool loop.
