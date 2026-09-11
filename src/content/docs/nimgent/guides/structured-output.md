---
title: Structured output
description: Get typed, validated data out of a model instead of free-form text.
---

Free-form model text is easy to print and hard to program against. Structured
output fixes that: you describe the shape of the data you want with a plain Nim
type, nimgent turns that type into a JSON Schema for the model, and you get back
a decoded, schema-validated Nim value — not a string you have to parse yourself.

```nim
type Recipe = object
  name: string
  servings: int
  ingredients: seq[string]

let recipe = generateObject[Recipe](
  model,
  prompt = "A weeknight lasagna.")

echo recipe.value.name        # "Weeknight lasagna"
echo recipe.value.servings    # 6
for ingredient in recipe.value.ingredients:
  echo "- ", ingredient
```

Under the hood, nimgent does four things for you:

1. Derives a JSON Schema from your type at compile time.
2. Asks the model for JSON that matches it (using the provider's native
   structured-output feature when one exists).
3. Validates what came back against the schema, locally repairing malformed
   JSON where it can.
4. Optionally sends the failure back to the model for another try, then decodes
   the validated JSON into your type.

Everything below builds on that loop. If a request fails after all of it, you
get an exception with the exact validation issues — nothing half-decoded slips
through.

## Optional fields

Use `Option[T]` when a field might not be present:

```nim
import std/options

type Recipe = object
  name: string
  servings: int
  notes: Option[string]
```

`notes` stays in the schema's `required` list but accepts `null`. That sounds
backwards, but it's what OpenAI's strict structured-output mode demands: every
declared property must be required, and optionality is expressed as "the value
may be null". A field marked `jsonOptional` (see below) behaves the same way.

## Constraining fields with pragmas

Types describe *what* a field is; pragmas describe *what makes it valid*. They
are compile-time annotations, so they cost nothing at runtime:

```nim
type Review = object
  headline {.jsonMinLength: 3, jsonMaxLength: 80.}: string
  score {.jsonMinimum: 1, jsonMaximum: 5.}: int
  url {.jsonPattern: "^https?://".}: string
  summary {.jsonDescription: "Two sentences, no marketing language".}: string
  spoilers {.jsonOptional.}: bool
```

| Pragma | Schema keyword |
| --- | --- |
| `jsonDescription` | `description` |
| `jsonPattern` | `pattern` |
| `jsonMinimum` / `jsonMaximum` | `minimum` / `maximum` |
| `jsonMinLength` / `jsonMaxLength` | `minLength` / `maxLength` |
| `jsonMinItems` / `jsonMaxItems` | `minItems` / `maxItems` |
| `jsonOptional` | removes the field from `required` |

The `description` pragma is worth using liberally — it's the cheapest way to
steer the model toward the output you actually want.

## Enums and unions

Nim enums become JSON string enums, so the model chooses from your values and
nothing else:

```nim
type Size = enum small, medium, large

type Order = object
  size: Size        # must be exactly "small", "medium", or "large"
  toppings: seq[string]
```

Enums with explicit string labels use those labels verbatim:

```nim
type Suit = enum
  clubs = "clubs", diamonds = "diamonds", hearts = "hearts", spades = "spades"
```

Variant objects (`case` objects) become discriminated unions. Each branch turns
into a `oneOf` entry, and the branch's own fields are only required when its
discriminator value is present:

```nim
type Shape = object
  case kind: ShapeKind
  of skCircle:
    radius: float
  of skRect:
    width: float
    height: float
```

This is how you model "one of several shapes of answer" — a support ticket that
is either a refund request or a bug report, for example — and have the type
system enforce the difference.

## Nested objects and containers

Nest plain objects freely, and use the standard containers you already use:

| Nim type | JSON Schema shape |
| --- | --- |
| `seq[T]`, `openArray[T]` | array of `<T>` |
| `array[N, T]` | array with exactly `N` items |
| `set[T]`, `HashSet[T]` | array with unique items |
| `Table[string, V]` | object with `<V>` values |
| `Option[T]` | `<T>` that may also be `null` |

Objects are derived with `"additionalProperties": false`, so the model cannot
invent fields you didn't declare. Generic types work too — `jsonSchema(Box[int])`
resolves `Box[T]` with `T = int`.

## Working without a type

Sometimes the schema only exists at runtime — loaded from a config file, built
dynamically, or shared with another service. Pass it directly and work with
`JsonNode`:

```nim
let schema = %*{
  "type": "object",
  "properties": {"answer": {"type": "string"}},
  "required": ["answer"]}

let result = generateObject(
  model,
  schema = schema,
  prompt = "Give a one-word answer.")

echo result.value["answer"].getStr
```

When you *do* have a type but the schema came from elsewhere, you can still
decode into it with `result.toObject[MyType]` — the same validation already ran,
so the decode either succeeds or raises with a clear message.

Schemas are checked before anything is sent to the provider. nimgent supports
the practical core of draft-07: `type`, `properties`, `required`, `enum`,
`const`, numeric bounds, string lengths and `pattern`, array bounds and
`items`, `uniqueItems`, `additionalProperties`, `anyOf`/`oneOf`/`allOf`, `not`,
`if`/`then`/`else`, and local `$ref`. Unsupported keywords (like `format`) and
external references fail fast with a list of the exact problems, rather than
silently producing garbage.

## Choosing how the model answers

There is more than one way to get JSON out of a model, and they have different
trade-offs. The `mode` parameter picks one — or lets nimgent choose:

| Mode | How the value arrives | When to use it |
| --- | --- | --- |
| `omAuto` *(default)* | Provider-native output if available, otherwise JSON parsed from the model's text | Almost always — best quality with a safe fallback |
| `omNative` | Provider-native structured output only | You want to *know* the schema was enforced, not parsed |
| `omJson` | Instructs the model to reply with raw JSON and extracts it from the text | Providers without native support, or schemas native mode can't express |
| `omTool` | A forced `submit` tool call whose arguments are the value | Models that behave better through tool calls |

The provider decides what "native" means — OpenAI's `response_format` with a
strict JSON schema, for example. Native mode is stricter than plain JSON Schema
(it rejects `oneOf`, `allOf`, `not`, and requires every property to be
required), so when a schema doesn't fit, `omAuto` quietly falls back to text
extraction while `omNative` raises an error telling you exactly which keyword
was the problem.

You can always check which path was taken:

```nim
if result.source == osNative:
  echo "schema enforced by the provider"
```

## When the model gets it wrong

Models produce malformed JSON and values that violate your constraints. nimgent
has three layers of defense, and they run in order:

**1. Local repair.** While parsing, nimgent closes unfinished brackets, string
literals, and partial booleans. This fixes the common "model ran out of tokens
mid-JSON" case without another model call. The result is flagged with
`result.locallyRepaired`.

**2. Validation.** The parsed value is checked against the schema. Anything
that doesn't match — wrong types, missing fields, enum values that don't exist,
numbers out of range — fails with issue paths like `$.ingredients[2]`.

**3. Model repairs.** With `maxRepairs` set above zero, the failed output and
its issues are fed back to the model for another attempt:

```nim
let recipe = generateObject[Recipe](
  model,
  prompt = "A weeknight lasagna.",
  maxRepairs = 2)   # up to 2 extra model turns
```

This costs extra calls, so leave it at `0` when the model is generally reliable
and turn it up for the flaky edges of your schema.

Truncation gets special treatment. A response cut off by `maxTokens` is
*rejected* by default — a repaired half-recipe is usually worse than no recipe.
When you'd rather have the partial value, opt in:

```nim
let recipe = generateObject[Recipe](
  model,
  prompt = "A long recipe.",
  truncation = otRepair)
```

## Streaming a partial object

For UIs that should show the object as it forms, `streamObject` reports partial
values through a callback as they arrive:

```nim
let recipe = streamObject[Recipe](
  model,
  prompt = "A weeknight lasagna.",
  onPartial = proc (partial: JsonNode): bool =
    if "name" in partial:
      stdout.write "\rname: " & partial["name"].getStr
      flushFile(stdout)
    true)   # return false to cancel
```

The partial value is best-effort parsed JSON, not a validated `Recipe` — fields
appear as the model produces them. The final value is validated (and repaired,
if you set `maxRepairs`) after the stream completes, and the `ObjectResult`
you get back is identical in shape to `generateObject`'s.

## What you get back

Both `generateObject` and `streamObject` return an `ObjectResult[T]`:

```nim
echo recipe.value.name          # the decoded T
echo recipe.usage.totalTokens   # token usage across all attempts
echo recipe.attempts            # 1 + repairs
echo recipe.source              # osNative, osText, or osTool
```

| Field | What it tells you |
| --- | --- |
| `value` | Your decoded value |
| `response` | The last provider response (text, content blocks, finish reason) |
| `usage` | Token usage aggregated across every attempt, including repairs |
| `repairs` | How many model repair turns were needed |
| `attempts` | Total provider calls made |
| `locallyRepaired` | Whether local JSON repair had to run |
| `source` | Whether the value came from native output, extracted text, or a tool call |

All the usual generation knobs work here too: `system` for instructions,
`maxTokens` for limits, `maxRetries` for transport retries, `abort` for
cancellation, and `providerOptions` for provider-specific settings. `name` and
`description` label the schema for native mode — the type name and an empty
description are used by default.

## Handling failures

When nothing succeeds, `generateObject` raises an `ObjectError`:

```nim
try:
  let recipe = generateObject[Recipe](model, prompt = "A weeknight lasagna.")
except ObjectError as e:
  for issue in e.issueDetails:
    echo issue.path, ": ", issue.message
  writeFile("last_failed.txt", e.raw)   # the model's raw output, for debugging
```

`issueDetails` carries one entry per validation problem with its JSON path, and
`raw` holds exactly what the model said — useful for filing bugs or building
your own retry UI. Related pages: [Streaming](/nimgent/guides/streaming/) for token-level
events, and [Tools and agents](/nimgent/guides/tools-and-agents/) — the same schema
derivation powers typed tool inputs.
