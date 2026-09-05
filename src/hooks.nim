## Lifecycle hooks: ephemeral JSON processes invoked by the agent.
##
## Same spawn protocol as external tools (runExtension). Fail-open on
## crash/timeout/bad JSON; only explicit {"allow": false} blocks a tool.

import std/[algorithm, json, os, strutils]
import config
import extensions

const
  DefaultTimeout = 30
  HookEvents* = ["pre_tool_call", "post_tool_call", "session_start", "session_end"]

type
  HookEvent* = enum
    hePreToolCall = "pre_tool_call"
    hePostToolCall = "post_tool_call"
    heSessionStart = "session_start"
    heSessionEnd = "session_end"

  Hook* = object
    name*: string
    event*: HookEvent
    tools*: seq[string] ## empty = all tools (pre/post only)
    command*: seq[string]
    timeoutSeconds*: int
    dir*: string

  HookDiscoverResult* = object
    hooks*: seq[Hook]
    warnings*: seq[string]

  HookOutcome* = object
    allowed*: bool
    reason*: string
    warnings*: seq[string]

proc parseEvent(s: string): tuple[ok: bool, event: HookEvent] =
  case s.strip.toLowerAscii
  of "pre_tool_call": (true, hePreToolCall)
  of "post_tool_call": (true, hePostToolCall)
  of "session_start": (true, heSessionStart)
  of "session_end": (true, heSessionEnd)
  else: (false, hePreToolCall)

proc parseHookManifest(path: string): tuple[ok: bool, hook: Hook, err: string] =
  var doc: JsonNode
  try:
    doc = parseJson(readFile(path))
  except CatchableError as e:
    return (false, Hook(), "invalid JSON: " & e.msg)
  if doc.isNil or doc.kind != JObject:
    return (false, Hook(), "manifest must be a JSON object")

  let name = if "name" in doc: doc["name"].getStr.strip else: ""
  if name.len == 0:
    return (false, Hook(), "missing name")
  if "event" notin doc:
    return (false, Hook(), "missing event")
  let ev = parseEvent(doc["event"].getStr)
  if not ev.ok:
    return (false, Hook(), "unknown event: " & doc["event"].getStr)
  if "command" notin doc or doc["command"].kind != JArray or doc["command"].len == 0:
    return (false, Hook(), "command must be a nonempty array")
  var command: seq[string] = @[]
  for item in doc["command"]:
    if item.kind != JString or item.getStr.len == 0:
      return (false, Hook(), "command entries must be nonempty strings")
    command.add item.getStr

  var tools: seq[string] = @[]
  if "tools" in doc:
    if doc["tools"].kind != JArray:
      return (false, Hook(), "tools must be an array")
    for item in doc["tools"]:
      if item.kind != JString or item.getStr.strip.len == 0:
        return (false, Hook(), "tools entries must be nonempty strings")
      tools.add item.getStr.strip.toLowerAscii

  var timeout = DefaultTimeout
  if "timeout_seconds" in doc:
    if doc["timeout_seconds"].kind != JInt:
      return (false, Hook(), "timeout_seconds must be an integer")
    timeout = max(1, doc["timeout_seconds"].getInt)

  result.ok = true
  result.hook = Hook(
    name: name,
    event: ev.event,
    tools: tools,
    command: command,
    timeoutSeconds: timeout,
    dir: path.parentDir)

proc addHooks(result: var HookDiscoverResult, root: string) =
  if not dirExists(root):
    return
  var dirs: seq[string] = @[]
  for kind, path in walkDir(root):
    if kind == pcDir and fileExists(path / "hook.json"):
      dirs.add path
  dirs.sort()
  for dir in dirs:
    let parsed = parseHookManifest(dir / "hook.json")
    if not parsed.ok:
      result.warnings.add "skipping " & dir & ": " & parsed.err
      continue
    var replaced = false
    for i in 0 ..< result.hooks.len:
      if result.hooks[i].name.toLowerAscii == parsed.hook.name.toLowerAscii:
        result.hooks[i] = parsed.hook
        replaced = true
        break
    if not replaced:
      result.hooks.add parsed.hook

proc discoverHooks*(workspace: string): HookDiscoverResult =
  ## Later roots override the same hook name: global → `.agent` → `.niminal`.
  addHooks(result, niminalConfigDir() / "hooks")
  let root = if dirExists(workspace): expandFilename(workspace) else: workspace
  addHooks(result, root / ".agent" / "hooks")
  addHooks(result, root / ".niminal" / "hooks")
  result.hooks.sort(proc(a, b: Hook): int =
    let byName = cmp(a.name.toLowerAscii, b.name.toLowerAscii)
    if byName != 0: byName else: cmp(a.dir, b.dir))

proc matchesTool(hook: Hook, toolName: string): bool =
  if hook.tools.len == 0:
    return true
  let lower = toolName.toLowerAscii
  for t in hook.tools:
    if t == lower:
      return true
  false

proc asExtension(hook: Hook): ExtensionTool =
  ExtensionTool(
    name: hook.name,
    description: "",
    command: hook.command,
    timeoutSeconds: hook.timeoutSeconds,
    inputSchema: %*{"type": "object"},
    dir: hook.dir)

proc runHooks*(hooks: openArray[Hook], event: HookEvent, payload: JsonNode,
               workspace: string, toolName = "",
               maxOutputBytes = 100_000): HookOutcome =
  ## Run matching hooks. Fail-open except explicit allow:false on pre_tool_call.
  result.allowed = true
  for hook in hooks:
    if hook.event != event:
      continue
    if event in {hePreToolCall, hePostToolCall} and not hook.matchesTool(toolName):
      continue
    let res = runExtension(hook.asExtension, payload, workspace, maxOutputBytes)
    if res.isError:
      result.warnings.add "hook '" & hook.name & "' failed: " & res.output
      continue
    var doc: JsonNode
    try:
      doc = parseJson(res.output)
    except CatchableError as e:
      result.warnings.add "hook '" & hook.name & "' failed: " & e.msg
      continue
    if event == hePreToolCall and doc.kind == JObject and "allow" in doc and
       doc["allow"].kind == JBool and not doc["allow"].getBool:
      result.allowed = false
      if "reason" in doc and doc["reason"].kind == JString and
         doc["reason"].getStr.len > 0:
        if result.reason.len == 0:
          result.reason = doc["reason"].getStr
        else:
          result.reason.add "; " & doc["reason"].getStr
      elif result.reason.len == 0:
        result.reason = "blocked by hook"
  if not result.allowed and result.reason.len == 0:
    result.reason = "blocked by hook"

proc sessionPayload*(sessionId, workspace: string): JsonNode =
  %*{"session_id": sessionId, "workspace": workspace}

proc preToolPayload*(tool: string, arguments: JsonNode): JsonNode =
  %*{
    "tool": tool,
    "arguments": if arguments.isNil: newJObject() else: arguments
  }

proc postToolPayload*(tool: string, arguments: JsonNode, output: string,
                      isError: bool): JsonNode =
  %*{
    "tool": tool,
    "arguments": if arguments.isNil: newJObject() else: arguments,
    "output": output,
    "is_error": isError
  }
