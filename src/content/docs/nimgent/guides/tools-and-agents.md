---
title: Tools and agents
description: Let the model call your functions, and keep the loop bounded.
---

A model can only produce text. It cannot look something up, write a file, or
call your API — unless you give it a way to ask. A **tool** is that way: a
function in your program that the model can request by name, with arguments it
invents from your description.

nimgent runs the exchange for you. When the model asks for a tool, nimgent
checks the arguments against the tool's schema, calls your function, and hands
the result back to the model so it can decide what to do next:

1. The model sees your tools and either answers or requests one, with arguments.
2. nimgent validates the arguments against the tool's JSON Schema.
3. Your function runs and returns a result.
4. The result becomes part of the conversation and the model continues.

This loop — model, tool, model — is what an **agent** is. This page covers
writing tools, then the agent that drives them.

## Define a typed tool

The simplest tool takes a typed input and returns a string:

```nim
type WeatherInput = object
  city: string

let weather = tool(
  "get_weather",
  "Get the current weather for a city",
  proc (_: ToolContext, input: WeatherInput): string =
    input.city & ": 16C and cloudy")
```

The name and description matter as much as the code. The model reads both to
decide *when* and *how* to call the tool — write the description like a prompt
for a new colleague, not a code comment:

```nim
tool(
  "get_weather",
  "Get current weather for a city name. Returns temperature and conditions. "
  "Use for any question about current conditions; do not use for forecasts.",
  handler)
```

From the type, nimgent derives a JSON Schema and advertises it to the provider.
Every schema feature described in [Structured output](/nimgent/guides/structured-output/)
applies to tool inputs too — enums, `Option`, pragmas like `jsonMinimum`,
nested objects, all of it.

## Returning richer results

Your tool can return anything Nim can serialize. A string is sent to the model
as-is; other values are serialized to JSON, and the same value is kept on the
result for your application to read:

```nim
type ForecastInput = object
  city: string

type Forecast = object
  city: string
  celsius: float
  raining: bool

tool(
  "get_forecast",
  "Get a structured forecast for a city",
  proc (_: ToolContext, input: ForecastInput): Forecast =
    Forecast(city: input.city, celsius: 16.0, raining: false))
```

The model sees `{"city": "Paris", "celsius": 16.0, "raining": false}` and can
reason over it; your code can later read the same structured value from
`response.steps` without re-parsing anything.

## When things go wrong

Two things can fail inside the loop, and both become *structured tool
failures* the model can read and react to — it will typically try again or
explain the problem to the user:

- **Bad arguments.** The model calls the tool with arguments that don't match
  the schema. nimgent rejects the call before your function runs.
- **Your function raises.** Any exception becomes a tool failure carrying the
  message.

You can also fail deliberately with a machine-readable error, which is better
than an exception because the model gets a stable code to reason about:

```nim
tool(
  "get_forecast",
  "Get a forecast for a known city",
  proc (_: ToolContext, input: ForecastInput): ToolResult =
    if input.city notin knownCities:
      return toolFailure(
        "unknown_city",
        "No forecast for '" & input.city & "'. Try one of: Paris, Tokyo.",
        retryable = false)
    # ... normal path
    )
```

The `message` is what the model sees; `details` (a `JsonNode`) is for your
application. `retryable = true` hints that a retry with different arguments
might work.

## Long-running tools

Every tool receives a `ToolContext` with per-call state. The most useful field
is `abort` — a callback that returns `true` when the caller has cancelled the
run. Check it between chunks of work so a slow tool can stop early:

```nim
proc search(context: ToolContext, input: SearchInput): SearchOutput =
  for page in searchPages(input.query):
    if context.abort():
      raise newException(CancelledError, "search cancelled")
    results.add page
```

The context also carries `callId`, `sessionId`, `turnId`, and `metadata` for
logging and correlation.

## Running tools in parallel

If the model asks for several tools at once, nimgent runs them one after
another by default. Pass `parallel = true` to tools that are safe to run
concurrently, and compile with `--threads:on` for the synchronous path:

```nim
tool("get_weather", "...", handler, parallel = true)
```

Async tools are a natural fit here — independent I/O overlaps without threads:

```nim
let forecast = tool(
  "get_forecast",
  "Get a forecast for a city",
  proc (_: ToolContext, input: ForecastInput): Future[Forecast] {.async.} =
    let response = await fetch(input.city)
    return parseForecast(response))
```

## Run a bounded agent

Calling `generateText` with tools runs one exchange. An **agent** packages the
model, instructions, and tools into a reusable object whose `run` keeps the
loop going until the model stops asking for tools:

```nim
import nimgent/agent

let researcher = newAgent(
  model,
  instructions = "You are a concise research assistant.",
  tools = @[weather, forecast],
  maxSteps = 5)

let response = researcher.run("Plan a picnic in Paris on Saturday.")
echo response.text
echo "turns: ", response.steps.len
```

`maxSteps` counts **model turns**, not tool calls — one turn can request
several tools. The default is 8; the cap exists so an accidental tool cycle
(ask → run → ask → run) cannot run forever. When the cap is hit, the response's
`finishReason` is `frStepLimit` and you get whatever the model produced so far.

Because an `Agent` is configuration, you can create it once and `run` it many
times; each run is independent. See
[Sessions](/nimgent/guides/sessions/) when runs should share a transcript.

## Steering tool use

By default the model chooses whether to call tools. To force a behavior, pass a
`toolChoice`:

```nim
researcher.run("Summarize the day.", toolChoice = toolChoiceRequired())
```

| Choice | Effect |
| --- | --- |
| `toolChoiceAuto()` | Model decides (default) |
| `toolChoiceRequired()` | The model must call *some* tool |
| `toolChoiceSpecific("get_weather")` | The model must call that tool |
| `toolChoiceNone()` | Tools are hidden; the model answers in text |

## Watch the loop as it happens

For UIs, `stream` gives you text deltas plus complete tool events:

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
    true)   # return false to cancel
```

The richer `AgentEvent` stream adds lifecycle boundaries — run start, each
step, tool results, errors — which is what the approval flow below builds on:

```nim
let events = researcher.events("Plan a picnic.")
while true:
  let (available, event) = await events.read()
  if not available: break
  case event.kind
  of aeToolCall:
    echo "→ ", event.call.name
  of aeToolResult:
    echo "← ", event.toolResult.output
  of aeStepFinish:
    echo "-- step ", event.step, " done"
  else:
    discard
discard await events.result
```

## Human approval

Some tools should not run just because the model asked. An approval policy
inspects every call before execution and returns `tamAllow`, `tamAsk`, or
`tamDeny`:

```nim
let cleaner = newAgent(
  model,
  instructions = "Tidy up the workspace.",
  tools = @[deleteTool],
  approvalPolicy = proc (_: int, call: ContentBlock, _: Tool): ToolApproval =
    if call.name == "delete_file":
      ToolApproval(mode: tamAsk, reason: "This deletes a file.")
    else:
      ToolApproval(mode: tamAllow))
```

When the policy says `tamAsk`, the event stream emits an
`aeToolApprovalRequired` event. Your application decides — here, auto-denying
after showing the user the reason:

```nim
of aeToolApprovalRequired:
  echo "\nApprove ", event.approval.toolName, "? ", event.approval.reason
  echo event.approval.input          # the arguments the model chose
  event.approval.deny()              # or .approve()
```

While the decision is pending, no other tool in that batch starts — sibling
side effects cannot race ahead of the user's answer. A denial is returned to
the model as a structured `approval_denied` failure, and the model continues
with that knowledge.

## Escape hatches

Two lower-level constructors cover the cases the typed helper does not:

- **`rawTool`** builds a tool from a hand-written `JsonNode` schema — useful
  when the schema is loaded at runtime or generated dynamically. Schema
  validation still runs before your `JsonNode`-based handler executes.
- **`hostedTool("web_search")`** declares a tool the *provider* executes
  server-side (Anthropic and Gemini's web search, for example). There is no
  local function at all; results arrive as normal content.

```nim
let webSearch = hostedTool("web_search")
let agent = newAgent(model, tools = @[webSearch], maxSteps = 4)
```

## Instrumenting runs

`RunCallbacks` adds observability to any run without touching the event
stream: `onRetry` for retries, `onToolStart`/`onToolFinish` with durations,
`onStepFinish` per model turn, and `onFinish` for the final response. Tools
and agents share the same failure philosophy as everything else in nimgent —
problems become structured values the model can handle; see
[Structured output](/nimgent/guides/structured-output/) for the type-first alternative
to free text.
