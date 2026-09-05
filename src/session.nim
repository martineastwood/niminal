## Append-only in-memory session with optional JSONL persistence.

import std/[algorithm, json, os, strutils, times]
from std/unicode import runeLen, runeSubStr
when defined(posix):
  from posix import fsync
import nimgent

type
  SessionEventKind* = enum
    sekUser = "user"
    sekAssistant = "assistant"
    sekToolResult = "tool_result"
    sekCompaction = "compaction"
    sekName = "name"

  SessionEvent* = object
    case kind*: SessionEventKind
    of sekUser, sekAssistant:
      message*: Message
      model*: string
      usage*: Usage
    of sekToolResult:
      toolId*: string
      toolOutput*: string
      toolError*: bool
      toolImages*: seq[ImageContent]
    of sekCompaction:
      summary*: string
      firstKeptIndex*: int
      tokensBefore*: int
    of sekName:
      sessionName*: string

  Session* = object
    id*: string
    path*: string
    workspace*: string    ## absolute workspace recorded in the JSONL header
    name*: string         ## last /name, not sent to the model
    events*: seq[SessionEvent]

  SessionInfo* = object
    id*: string
    mtime*: Time
    preview*: string      ## first user message, single clipped line
    name*: string         ## /name if set
    workspace*: string

const
  sessionListLimit* = 20  ## picker / tab-complete; explicit /resume ID is uncapped

proc validSessionId*(id: string): bool =
  if id.len == 0: return false
  for ch in id:
    if not ch.isAlphaNumeric and ch notin {'-', '_'}:
      return false
  true

proc sessionFileError*(id, path: string): string =
  if not validSessionId(id): "Invalid session ID."
  elif not fileExists(path): "Session not found: " & id
  else: ""

proc imageJson(img: ImageContent): JsonNode =
  result = %*{"type": "image", "mimeType": img.mimeType}
  if img.path.len > 0:
    result["path"] = %img.path
  elif img.data.len > 0:
    result["data"] = %img.data

proc parseImage(node: JsonNode): ImageContent =
  ImageContent(mimeType: node.getOrDefault("mimeType").getStr,
               data: node.getOrDefault("data").getStr,
               path: node.getOrDefault("path").getStr)

proc eventJson(event: SessionEvent): JsonNode =
  result = %*{"type": $event.kind}
  case event.kind
  of sekUser, sekAssistant:
    result["role"] = %($event.message.role)
    result["content"] = newJArray()
    for part in event.message.content:
      case part.kind
      of ckText:
        result["content"].add %*{"type": "text", "text": part.text}
      of ckToolUse:
        var tu = %*{"type": "tool_use", "id": part.id,
          "name": part.name, "input": part.input}
        if part.parseError.len > 0:
          tu["parse_error"] = %part.parseError
        result["content"].add tu
      of ckThinking:
        result["content"].add %*{"type": "thinking", "thinking": part.thinking,
          "signature": part.signature}
      of ckToolResult:
        var tr = %*{"type": "tool_result", "tool_use_id": part.toolUseId,
          "content": part.output, "is_error": part.isError}
        if part.images.len > 0:
          var imgs = newJArray()
          for img in part.images:
            imgs.add imageJson(img)
          tr["images"] = imgs
        result["content"].add tr
      of ckImage:
        result["content"].add imageJson(part.toImage)
    if event.kind == sekAssistant:
      if event.model.len > 0:
        result["model"] = %event.model
      if event.usage.inputTokens > 0 or event.usage.outputTokens > 0 or
          event.usage.cacheReported:
        result["usage"] = %*{
          "input_tokens": event.usage.inputTokens,
          "output_tokens": event.usage.outputTokens,
          "cache_read_tokens": event.usage.cacheReadTokens,
          "cache_write_tokens": event.usage.cacheWriteTokens,
          "cache_reported": event.usage.cacheReported
        }
  of sekToolResult:
    result["id"] = %event.toolId
    result["output"] = %event.toolOutput
    result["is_error"] = %event.toolError
    if event.toolImages.len > 0:
      var imgs = newJArray()
      for img in event.toolImages:
        imgs.add imageJson(img)
      result["images"] = imgs
  of sekCompaction:
    result["summary"] = %event.summary
    result["first_kept_index"] = %event.firstKeptIndex
    result["tokens_before"] = %event.tokensBefore
  of sekName:
    result["name"] = %event.sessionName

proc parseImages(node: JsonNode): seq[ImageContent] =
  if node.isNil or node.kind != JArray: return
  for item in node:
    result.add parseImage(item)

proc parseBlock(node: JsonNode): ContentBlock =
  case node["type"].getStr
  of "text": text(node["text"].getStr)
  of "tool_use":
    toolUse(node["id"].getStr, node["name"].getStr, node["input"],
      node.getOrDefault("parse_error").getStr)
  of "thinking":
    ContentBlock(kind: ckThinking, thinking: node["thinking"].getStr,
      signature: if "signature" in node: node["signature"].getStr else: "")
  of "tool_result":
    toolResult(node["tool_use_id"].getStr, node["content"].getStr,
      if "is_error" in node: node["is_error"].getBool else: false,
      parseImages(node.getOrDefault("images")))
  of "image":
    image(parseImage(node))
  else:
    raise newException(ValueError, "unknown content block")

proc parseEvent(node: JsonNode): SessionEvent =
  case node["type"].getStr
  of "user":
    var msg = Message(role: roleUser, content: @[])
    for part in node["content"]:
      msg.content.add parseBlock(part)
    SessionEvent(kind: sekUser, message: msg)
  of "assistant":
    var msg = Message(role: roleAssistant, content: @[])
    for part in node["content"]:
      msg.content.add parseBlock(part)
    var usage: Usage
    let model = node.getOrDefault("model").getStr
    if "usage" in node:
      let u = node["usage"]
      usage.inputTokens = u.getOrDefault("input_tokens").getInt
      usage.outputTokens = u.getOrDefault("output_tokens").getInt
      usage.cacheReadTokens = u.getOrDefault("cache_read_tokens").getInt
      usage.cacheWriteTokens = u.getOrDefault("cache_write_tokens").getInt
      usage.cacheReported = u.getOrDefault("cache_reported").getBool
    SessionEvent(kind: sekAssistant, message: msg, model: model, usage: usage)
  of "tool_result":
    SessionEvent(kind: sekToolResult, toolId: node["id"].getStr,
      toolOutput: node["output"].getStr, toolError: node["is_error"].getBool,
      toolImages: parseImages(node.getOrDefault("images")))
  of "compaction":
    SessionEvent(kind: sekCompaction,
      summary: node.getOrDefault("summary").getStr,
      firstKeptIndex: node.getOrDefault("first_kept_index").getInt,
      tokensBefore: node.getOrDefault("tokens_before").getInt)
  of "name":
    SessionEvent(kind: sekName, sessionName: node.getOrDefault("name").getStr)
  else:
    raise newException(ValueError, "unknown session event")

proc initSession*(path = "", id = ""): Session =
  result.id = if id.len > 0: id else: $int(epochTime() * 1_000_000)
  result.path = path
  if path.len == 0 or not fileExists(path): return
  var first = true
  for line in lines(path):
    if line.strip.len == 0: continue
    try:
      let node = parseJson(line)
      if first and node.getOrDefault("type").getStr == "session":
        result.workspace = node.getOrDefault("workspace").getStr
        first = false
        continue
      first = false
      result.events.add parseEvent(node)
      if result.events[^1].kind == sekName:
        result.name = result.events[^1].sessionName
    except CatchableError:
      # A final partial JSONL line is expected after an interrupted write.
      break

proc persistSessionFile(file: File) =
  file.flushFile()
  when defined(posix):
    discard fsync(file.getFileHandle().cint)

proc append*(session: var Session, event: SessionEvent) =
  session.events.add event
  if session.path.len == 0: return
  let parent = session.path.parentDir
  if parent.len > 0: createDir(parent)
  let writeHeader = session.workspace.len > 0 and
    (not fileExists(session.path) or getFileSize(session.path) == 0)
  let file = open(session.path, fmAppend)
  if writeHeader:
    file.writeLine(%*{"type": "session", "workspace": session.workspace})
  file.writeLine(eventJson(event))
  persistSessionFile(file)
  file.close()

proc addUserMessage*(session: var Session, content: string) =
  session.append SessionEvent(kind: sekUser, message: userMessage(content))

proc addUserMessage*(session: var Session, content: seq[ContentBlock]) =
  session.append SessionEvent(kind: sekUser, message: userMessage(content))

proc addAssistantResponse*(session: var Session, response: ProviderResponse) =
  session.append SessionEvent(kind: sekAssistant,
    message: Message(role: roleAssistant, content: response.content),
    model: response.model, usage: response.usage)

proc addCompaction*(session: var Session, summary: string, firstKeptIndex: int,
                    tokensBefore: int) =
  session.append SessionEvent(kind: sekCompaction, summary: summary,
    firstKeptIndex: firstKeptIndex, tokensBefore: tokensBefore)

proc setName*(session: var Session, name: string) =
  session.name = name.strip
  session.append SessionEvent(kind: sekName, sessionName: session.name)

proc lastAssistant*(session: Session): tuple[found: bool, model: string, usage: Usage] =
  for i in countdown(session.events.high, 0):
    if session.events[i].kind == sekAssistant:
      return (true, session.events[i].model, session.events[i].usage)
  (false, "", Usage())

proc addToolResult*(session: var Session, call: ContentBlock, result: string,
                    isError: bool, images: seq[ImageContent] = @[]) =
  session.append SessionEvent(kind: sekToolResult, toolId: call.id,
    toolOutput: result, toolError: isError, toolImages: images)

proc messagesFrom*(session: Session, startIdx: int): seq[Message] =
  ## Convert events from `startIdx` into provider messages (skip compaction).
  let lo = max(0, startIdx)
  for i in lo ..< session.events.len:
    let event = session.events[i]
    case event.kind
    of sekUser, sekAssistant:
      result.add event.message
    of sekToolResult:
      if result.len == 0 or result[^1].role != roleUser:
        result.add Message(role: roleUser, content: @[])
      result[^1].content.add toolResult(event.toolId, event.toolOutput,
        event.toolError, event.toolImages)
    of sekCompaction, sekName:
      discard

proc messages*(session: Session): seq[Message] =
  ## Full raw session as provider messages (ignores compaction boundaries).
  session.messagesFrom(0)

proc messagesForModel*(session: Session): seq[Message] =
  ## Context sent to the model: latest summary (if any) + kept recent events.
  var startIdx = 0
  var summary = ""
  for i in countdown(session.events.high, 0):
    if session.events[i].kind == sekCompaction:
      summary = session.events[i].summary
      startIdx = session.events[i].firstKeptIndex
      break
  if summary.len > 0:
    result.add userMessage(
      "The conversation history before this point was compacted into the following summary:\n" &
      "<summary>\n" & summary & "\n</summary>")
  result.add session.messagesFrom(startIdx)

proc loadSession*(sessionDir: string, id = "", workspace = ""): Session =
  ## New session when `id` is empty; otherwise resume an existing JSONL file.
  if id.len == 0:
    result = initSession()
    result.path = sessionDir / (result.id & ".jsonl")
    result.workspace = workspace
    return
  let path = sessionDir / (id & ".jsonl")
  let err = sessionFileError(id, path)
  if err.len > 0:
    raise newException(ValueError, err)
  initSession(path, id)

proc tryLoadSession*(sessionDir: string, id: string):
                    tuple[ok: bool, session: Session, err: string] =
  let path = sessionDir / (id & ".jsonl")
  let err = sessionFileError(id, path)
  if err.len > 0:
    (false, initSession(), err)
  else:
    (true, initSession(path, id), "")

proc relativeAge*(t: Time, now = getTime()): string =
  let secs = max(0'i64, now.toUnix - t.toUnix)
  if secs < 60: return "just now"
  if secs < 3600: return $(secs div 60) & "m ago"
  if secs < 86400: return $(secs div 3600) & "h ago"
  if secs < 86400 * 7: return $(secs div 86400) & "d ago"
  $(secs div (86400 * 7)) & "w ago"

proc clipPreview(text: string, maxRunes = 48): string =
  var one = text.strip
  let nl = one.find('\n')
  if nl >= 0: one = one[0 ..< nl].strip
  if runeLen(one) <= maxRunes: return one
  runeSubStr(one, 0, maxRunes - 1) & "…"

proc belongsToWorkspace(fileWorkspace, wanted: string): bool =
  wanted.len == 0 or fileWorkspace == wanted

proc peekFile(mtime: Time, id, path: string): SessionInfo =
  result = SessionInfo(id: id, mtime: mtime, preview: "(empty)")
  if not fileExists(path): return
  var gotPreview = false
  for line in lines(path):
    let stripped = line.strip
    if stripped.len == 0: continue
    try:
      let node = parseJson(stripped)
      case node.getOrDefault("type").getStr
      of "session":
        result.workspace = node.getOrDefault("workspace").getStr
      of "name":
        result.name = node.getOrDefault("name").getStr
      of "user":
        if gotPreview: continue
        let content = node.getOrDefault("content")
        if content.isNil or content.kind != JArray: continue
        for part in content:
          if part.getOrDefault("type").getStr == "text":
            let t = part.getOrDefault("text").getStr
            if t.strip.len == 0: continue
            result.preview = clipPreview(t)
            gotPreview = true
            break
      else:
        discard
    except CatchableError:
      continue

proc collectSessionFiles(sessionDir: string): seq[tuple[mtime: Time, id, path: string]] =
  if sessionDir.len == 0 or not dirExists(sessionDir):
    return
  for kind, path in walkDir(sessionDir):
    if kind != pcFile: continue
    let name = path.extractFilename
    if not name.endsWith(".jsonl"): continue
    let id = name[0 ..< name.len - 6]
    if not validSessionId(id): continue
    result.add (getLastModificationTime(path), id, path)
  result.sort do (a, b: tuple[mtime: Time, id, path: string]) -> int:
    result = cmp(b.mtime, a.mtime)
    if result == 0: result = cmp(a.id, b.id)

proc peekSession*(sessionDir, id: string): SessionInfo =
  if not validSessionId(id): return
  let path = sessionDir / (id & ".jsonl")
  if not fileExists(path): return
  peekFile(getLastModificationTime(path), id, path)

proc listSessions*(sessionDir: string, workspace = "",
                   limit = sessionListLimit): seq[SessionInfo] =
  ## Newest mtime first. Preview is the first user message, clipped.
  ## `workspace` hides sessions from other projects.
  for e in collectSessionFiles(sessionDir):
    let info = peekFile(e.mtime, e.id, e.path)
    if not belongsToWorkspace(info.workspace, workspace):
      continue
    result.add info
    if limit > 0 and result.len >= limit:
      break

proc listSessionIds*(sessionDir: string, workspace = ""): seq[string] =
  ## Session ids in `sessionDir`, newest mtime first. Uncapped.
  for info in listSessions(sessionDir, workspace, limit = 0):
    result.add info.id

proc sessionLabel*(info: SessionInfo, now = getTime()): string =
  let title = if info.name.len > 0: info.name else: info.preview
  relativeAge(info.mtime, now) & "  " & title

proc sessionListLine*(info: SessionInfo, currentId = "", now = getTime()): string =
  result = sessionLabel(info, now) & "  #" & info.id
  if info.id == currentId:
    result.add "  (current)"
