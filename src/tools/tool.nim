## Tool dispatcher: registers tools and executes them by name.

import std/[json, tables]
import nimgent

type
  ToolResult* = object
    output*: string
    isError*: bool
    images*: seq[ImageContent]

  ToolProc* = proc(input: JsonNode): ToolResult

  CancelCheck* = proc (): bool {.closure.}

  ToolRegistry* = object
    tools: OrderedTable[string, ToolProc]
    definitions: seq[ToolDefinition]

var activeCancel {.threadvar.}: CancelCheck

proc cancelRequested*(): bool =
  ## True when the in-flight `execute` should abort (Ctrl-C, Escape).
  not activeCancel.isNil and activeCancel()

proc register*(reg: var ToolRegistry, def: ToolDefinition, fn: ToolProc) =
  if reg.tools.len == 0:
    reg.tools = initOrderedTable[string, ToolProc]()
  reg.tools[def.name] = fn
  reg.definitions.add def

proc definitions*(reg: ToolRegistry): seq[ToolDefinition] =
  reg.definitions

proc execute*(reg: ToolRegistry, name: string, input: JsonNode,
              shouldCancel: CancelCheck = nil): ToolResult =
  if name notin reg.tools:
    return ToolResult(output: "Unknown tool: " & name, isError: true)
  let prev = activeCancel
  activeCancel = shouldCancel
  defer: activeCancel = prev
  try:
    reg.tools[name](input)
  except CatchableError as e:
    ToolResult(output: e.msg, isError: true)
