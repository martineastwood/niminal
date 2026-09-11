---
title: Quickstart
description: Make your first request, then your first agent — the short path.
---

nimgent is a Nim client for talking to large language models. You pick a
provider, bind a model, and call functions that send prompts and receive
responses. This page walks through your first request, then builds it into a
small agent — the two ideas everything else in nimgent builds on.

## Install

Install the package with Nimble:

```sh
nimble install nimgent
```

For a sibling checkout during development, add the source directory to
`nim.cfg`:

```text
--path:"../nimgent/src"
```

You will also need an API key from a provider. This page uses OpenRouter, but
any of them works — the code is identical apart from the import and the
constructor.

## Your first request

Three lines of setup, one call:

```nim
import std/os
import nimgent
import nimgent/providers/openrouter

let model = openRouter(getEnv("OPENROUTER_API_KEY")).model(
  "deepseek/deepseek-v4-flash-0731")

let response = generateText(model, prompt = "Say hello in one sentence.")
echo response.text
```

Save that as `hello.nim` and run it:

```sh
OPENROUTER_API_KEY=... nim c -r hello.nim
```

What just happened, piece by piece:

- `openRouter(apiKey)` creates a **provider** — the adapter that knows how to
  talk to one API.
- `.model("...")` binds a specific model on that provider into a
  `LanguageModel` you can pass around.
- `generateText(model, prompt = ...)` sends the prompt and returns a
  `ProviderResponse`. The model's reply is `response.text`.

The prompt is the only required input, but two more are used constantly:

```nim
let response = generateText(
  model,
  system = "You are a terse systems programmer.",  # standing instructions
  prompt = "Explain what a file descriptor is.")

echo response.usage.totalTokens   # what the call cost
echo response.finishReason        # why the model stopped (frStop = normal)
```

`system` sets the model's standing instructions — role, tone, rules — while the
prompt is the actual question of this turn. Keep system text short and let it
apply across many requests.

## Async, the same thing

The blocking `generateText` is a convenience wrapper. The async primitive is
the same call with `Async` appended and an `await` added — use it in servers or
anywhere with an event loop:

```nim
import std/[asyncdispatch, os]
import nimgent
import nimgent/providers/openrouter

let model = openRouter(getEnv("OPENROUTER_API_KEY")).model(
  "deepseek/deepseek-v4-flash-0731")

proc main() {.async.} =
  let response = await generateTextAsync(model, prompt = "Hello")
  echo response.text

waitFor main()
```

Every blocking helper in nimgent has an `...Async` twin. The rest of this page
sticks to the blocking form to keep examples short.

## Pick a provider

The provider is confined to two lines — the import and the constructor. Change
those, keep everything else:

```nim
import nimgent/providers/[anthropic, google, hyper, openai, openrouter]

let openaiModel = openAI(getEnv("OPENAI_API_KEY")).model("gpt-4o-mini")
let claudeModel = anthropic(getEnv("ANTHROPIC_API_KEY")).model("claude-sonnet-4-6")
let geminiModel = google(getEnv("AI_STUDIO_API_KEY")).model("gemini-3.5-flash-lite")
```

All adapters expose the same `LanguageModel`, and responses are normalized to
one shape, so switching providers is not a rewrite. Provider-specific settings
belong in `providerOptions` or the raw `options` escape hatch — see
[Providers](/nimgent/guides/providers/) for the typed form.

## Your first agent

A single request is one exchange: prompt in, answer out. Many real tasks need
more — look something up, then reason about it, then answer. nimgent lets the
model request **tools**: ordinary Nim functions the model can call by name,
with arguments it invents. The loop of *model asks → your code runs → model
continues* is an agent.

Define a tool by giving nimgent a typed function:

```nim
type WeatherInput = object
  city: string

let weather = tool(
  "get_weather",
  "Get the current weather for a city",
  proc (_: ToolContext, input: WeatherInput): string =
    input.city & ": 16C and cloudy")
```

The type becomes a JSON Schema the provider enforces; the string the tool
returns goes back to the model. Then hand the tool to `generateText` and raise
the step limit so the loop has room to work:

```nim
let response = generateText(
  model,
  prompt = "What's the weather like in Paris?",
  tools = @[weather],
  maxSteps = 5)

echo response.text
```

The model calls `get_weather` with `{"city": "Paris"}`, your function runs, and
the model folds the result into its answer. `maxSteps` counts model turns, not
tool calls — it is the safety cap that stops a tool loop from running forever.

When the exchange grows beyond one prompt, package the model, instructions, and
tools into a reusable `Agent` instead:

```nim
import nimgent/agent

let researcher = newAgent(
  model,
  instructions = "You are a concise research assistant.",
  tools = @[weather],
  maxSteps = 5)

let answer = researcher.run("Should I bring an umbrella to Paris tomorrow?")
echo answer.text
echo "model turns: ", answer.steps.len
```

The agent is configuration, not state: create it once and run it many times.
Add a `Session` when runs should remember each other — see
[Sessions](/nimgent/guides/sessions/).

## Where to next

- [Tools and agents](/nimgent/guides/tools-and-agents/) — richer tools, errors the
  model can handle, approval gates, and the event stream for UIs.
- [Structured output](/nimgent/guides/structured-output/) — get typed, validated Nim
  values back instead of prose.
- [Streaming](/nimgent/guides/streaming/) — render tokens as they arrive.
- [Core API](/nimgent/reference/core-api/) — the types and entry points, on one page.
