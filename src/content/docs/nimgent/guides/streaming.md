---
title: Streaming
description: Render tokens as they arrive instead of waiting for the full response.
---

A model can take many seconds to answer. Without streaming, your program waits
for the entire response before showing anything. **Streaming** delivers the
answer in pieces as the model produces them — which is what makes chat UIs,
long generations, and live tool activity feel responsive.

nimgent streams through a callback. You pass a proc, nimgent calls it with each
event, and when the stream ends you still get the complete, normalized
`ProviderResponse` — the same object `generateText` would have returned, with
`text`, `usage`, and `finishReason` assembled from the stream.

## Stream text

The streaming counterpart to `generateText` is `streamText`, which takes an
`onEvent` callback:

```nim
import std/[os, strutils]
import nimgent
import nimgent/providers/openai

let model = openAI(getEnv("OPENAI_API_KEY")).model("gpt-4o-mini")

let response = streamText(
  model,
  prompt = "Count to three.",
  onEvent = proc (event: StreamEvent): bool =
    if event.kind == seTextDelta:
      stdout.write event.text
      flushFile(stdout)
    true)

echo ""
echo response.finishReason   # the full response is still available
```

Two details worth noticing in that callback:

- It **returns a bool**. Return `true` to keep going, `false` to cancel the
  stream early — the run then raises `CancelledError`.
- Events arrive for more than text. The case you don't handle is skipped, so a
  minimal callback is fine; handle more kinds when you need them.

## The event kinds

`StreamEvent` has one `kind`, and the fields it carries depend on that kind:

| Kind | Carries | What it means |
| --- | --- | --- |
| `seTextDelta` | `text` | A fragment of the visible answer |
| `seThinkingDelta` | `text` | A fragment of the model's reasoning (reasoning models) |
| `seToolCallDelta` | `toolCallId`, `toolName`, `toolArgs` | A fragment of a tool call's arguments |
| `seWake` | — | A file descriptor you're watching became readable |
| `seFinished` | — | The stream ended (emitted once, at the end) |

Thinking deltas are useful to show a "reasoning…" panel; tool-call deltas let a
UI show what the agent is about to do before it finishes asking. `seWake` is
specialized: when you pass a `wakeFd` (a file descriptor, stdin is `0`), nimgent
emits `seWake` whenever it becomes readable — a way to inject input or trigger
cancellation mid-stream on the local side.

## Async streaming

The async form is the primitive; `streamText` is a thin wrapper. In a server or
any program with an event loop, prefer it:

```nim
let response = await streamTextAsync(
  model,
  prompt = "Write a short haiku.",
  onEvent = proc (event: StreamEvent): bool =
    if event.kind == seTextDelta:
      stdout.write event.text
    true)
```

Keep callbacks fast — they run on your event loop, so a slow callback delays
everything else. Buffer or hand off expensive work (parsing, rendering,
database writes) to the surrounding application rather than doing it inline.

## Streaming an agent

Agents and sessions stream with the same callback shape. Tool calls appear as
they're requested, which is the quickest way to make an agent legible to a
user:

```nim
let response = researcher.stream(
  "Plan a picnic in Paris.",
  proc (event: StreamEvent): bool =
    case event.kind
    of seTextDelta:
      stdout.write event.text
    of seToolCallDelta:
      echo "\n→ ", event.toolName, "(", event.toolArgs, ")"
    else:
      discard
    true)
```

When text deltas alone aren't enough — approvals, step boundaries, run start
and finish — agents also emit a richer normalized event stream:

```nim
let events = researcher.events("What should I deploy?")
while true:
  let (available, event) = await events.read()
  if not available: break
  handle(event)   # event.kind: aeRunStart, aeTextDelta, aeToolCall, ...

let response = await events.result
```

See [Tools and agents](/nimgent/guides/tools-and-agents/) for the full event list and
approval handling, and [Structured output](/nimgent/guides/structured-output/) for
streaming a *validated object* as it forms.
