---
title: Examples
description: Runnable examples shipped with the nimgent repository.
---

The repository keeps complete examples in
[`examples/`](https://github.com/martineastwood/nimgent/tree/main/examples).

| Example | Demonstrates |
| --- | --- |
| `generate_text.nim` | Basic generation |
| `stream_text.nim` | Incremental text output |
| `async_generate.nim` | Concurrent async requests |
| `tool_call.nim` | A typed local tool |
| `agent.nim` | A bounded reusable agent |
| `agent_events.nim` | Lifecycle events and approval |
| `structured_output.nim` | Typed JSON output |
| `stream_object.nim` | Partial structured output |
| `session.nim` | Persisted multi-turn conversations |
| `embeddings.nim` | Batch embeddings and similarity |
| `provider_options.nim` | Typed provider settings |
| `wrap_provider.nim` | Request and response middleware |

Run one from the repository root:

```sh
OPENAI_API_KEY=... nim c -r examples/stream_text.nim
```

The provider smoke tests use their corresponding credentials:

```sh
ANTHROPIC_API_KEY=... nim c -r examples/anthropic_smoke.nim
AI_STUDIO_API_KEY=... nim c -r examples/google_smoke.nim
```
