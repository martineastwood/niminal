---
title: Sessions
description: Give an agent a memory — a transcript that survives across runs.
---

By default, every run is a stranger. Ask about the weather, then ask what to
wear, and the second call has no idea what the first one said — the model only
sees the messages you send it. A **session** fixes that by owning the
conversation transcript: every prompt, answer, and tool result is appended to
it, and every new run includes it.

Two things stay separate:

- The **`Agent`** is configuration — model, instructions, tools. It is
  reusable and stateless.
- The **`Session`** is one conversation — the transcript, turn count, and
  total usage. You create it from an agent, and each `run` appends to it.

```nim
import nimgent/[agent, session]

let conversation = newSession(researcher, id = "weather-demo")

let first = conversation.run("What's the weather like in Paris?")
echo first.text

let second = conversation.run("What should I wear?")
echo second.text    # the model knows the forecast from the first turn
```

The id is optional; nimgent generates one when omitted. It is passed through to
providers that support server-side sessions and appears in events and logs, so
it's worth setting when you'll want to correlate runs later.

## What a session records

Each turn appends a small set of events to an append-only transcript: the user
prompt, each assistant step (a model turn, including tool calls), tool results,
and a final turn-finished marker. You can read them:

```nim
for event in conversation.events:
  case event.kind
  of sekUser, sekAssistant:
    echo $event.message.role, ": ", textContent(event.message.content)
  else:
    discard

echo conversation.turns        # completed model turns
echo conversation.totalUsage.inputTokens   # usage across every turn
```

Failures are observable too. A cancelled or failed turn records a
`sekTurnFailed` event with the error, but nothing incomplete ever enters the
model-facing transcript — the next run starts from the last *completed* state,
not from a half-written exchange.

## Continue from a snapshot

A session can be serialized to JSON and restored later — that's how you
persist a conversation across process restarts, or hand it to a background
worker:

```nim
let snapshot = conversation.sessionJsonString

# later, possibly in another process:
let resumed = sessionFromJson(researcher, snapshot)
let next = resumed.run("What should I wear? Keep it brief.")
```

The snapshot contains the transcript and derived state — not your API key, not
tool callbacks. That's deliberate: a session file should be safe to store
without leaking credentials, and restoring requires you to supply the agent
again, so the tools and instructions that run are always ones you chose. The
JSON carries a schema version, and restoring a snapshot from a different
version fails loudly rather than guessing.

## Stream a session

Sessions support the same streaming forms as agents. Text arrives live, but the
transcript is only committed after the run completes — so a cancelled stream
leaves no partial turn behind:

```nim
let response = conversation.stream(
  "Summarize the weather.",
  proc (event: StreamEvent): bool =
    if event.kind == seTextDelta:
      stdout.write event.text
    true)
```

For UIs that also need lifecycle boundaries and approval prompts, use the
`AgentEvent` callback overload or the pull-based stream:

```nim
let events = conversation.events("What changed since yesterday?")
while true:
  let (available, event) = await events.read()
  if not available: break
  handle(event)

let response = await events.result
```

## Start over

`reset` clears the transcript and counters but keeps the agent and the session
id — useful for a "new conversation" button:

```nim
conversation.reset()
```

If instead you want a *fresh conversation* with a new id, create a new session
with `newSession(researcher)`.

## Agents, sessions, and models

A last note on how the pieces fit, since this is where newcomers usually trip:

- A `LanguageModel` is bound to one provider; it is the "who answers".
- An `Agent` wraps a model with instructions, tools, and limits; it is the
  "how it behaves".
- A `Session` wraps an agent with a transcript; it is the "what has been said".

You can run a model directly (`generateText`), run an agent (`agent.run`), or
run a session (`conversation.run`) — each layer just adds state to the one
below it. Pick the smallest one that does the job, and add layers when the
conversation actually needs them.
