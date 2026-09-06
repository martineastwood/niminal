## Event-driven TUI on niminal's own terminal layer.
##
## When idle, blocks on select()/WaitForSingleObject — ~0% CPU.
## When streaming, wakes per token chunk. No fixed-frequency loop.

import std/[os, strutils, json]
from std/unicode import Rune, runeLen, runeSubStr, fastRuneAt
import nimgent
import ../session
import ../config
import markdown
import keys
import term
import input
import ansi
import diff
import theme
import ../commands
import ../workspace
import ../images

## User turns: accent rail + panel row (OpenCode-style). `\e[K` at paint
## fills the rest of the row while the background is still open.
proc userCardLines(body: string): seq[string] =
  let rail = userRail()
  result.add rail
  if body.len == 0:
    result.add rail
  else:
    for line in body.splitLines:
      result.add rail & line
  result.add rail
  result.add ""

proc wrapLineForWidth(line: string, w: int): seq[string] =
  if ansiVisibleWidth(line) <= w:
    return @[line]
  let (prefix, body, gutter) = peelBoxGutter(line)
  if gutter > 0:
    let inner = max(1, w - gutter)
    for part in wrapAnsi(body, inner):
      result.add prefix & part
  else:
    for part in wrapAnsi(line, w):
      result.add part

proc wrapLineCount*(line: string, w: int): int =
  wrapLineForWidth(line, w).len

proc padRight(s: string, width: int): string =
  if s.len >= width: s else: s & " ".repeat(width - s.len)

proc clipAscii(s: string, width: int): string =
  if width <= 0: return ""
  if s.len <= width: return s
  if width == 1: return "…"
  s[0 ..< width - 1] & "…"

proc formatCommandMenuLine*(usage, description: string, selected: bool,
                            labelWidth, width: int): string =
  ## OpenCode-style row: panel, two columns, full-width selection.
  ## Background stays open so the TUI `\e[K` fills the rest of the row.
  let t = currentTheme
  let label = padRight(usage, max(labelWidth, usage.len))
  let descWidth = max(0, width - 1 - label.len - 2)
  let desc = clipAscii(description, descWidth)
  if selected:
    result = t.selectedBg & t.selectedFg & " " & label & "  " & desc
  else:
    result = t.panelBg & t.text & " " & label & t.muted & "  " & desc

proc formatCommandMenu*(suggestions: openArray[string], selected, rows, width: int,
                        workspace = getCurrentDir(), sessionDir = ""): seq[string] =
  if rows <= 0 or suggestions.len == 0:
    return
  let sel = clamp(selected, 0, suggestions.high)
  let first = if sel >= rows: sel - rows + 1 else: 0
  var labelWidth = 0
  for i in first ..< min(suggestions.len, first + rows):
    labelWidth = max(labelWidth, suggestions[i].len)
  for i in first ..< min(suggestions.len, first + rows):
    result.add formatCommandMenuLine(suggestions[i],
      commandSuggestionDescription(suggestions[i], workspace, sessionDir), i == sel,
      labelWidth, width)

proc highlightSlashCommand*(input: string, workspace = getCurrentDir()): string =
  ## Highlight the command token and flag malformed arguments in red.
  if input.strip.len == 0 or not input.strip.startsWith("/"):
    return input
  var tokenStart = 0
  while tokenStart < input.len and input[tokenStart] in {' ', '\t'}:
    inc tokenStart
  var tokenEnd = tokenStart
  while tokenEnd < input.len and input[tokenEnd] notin {' ', '\t'}:
    inc tokenEnd
  let t = currentTheme
  let command = input[tokenStart ..< tokenEnd]
  let parsed = parseSlash(command, workspace)
  let known = parsed.kind notin {slNone, slError}
  let commandColor = if known: t.boldAccent else: t.boldError
  result = input[0 ..< tokenStart] & t.paint(commandColor, command)
  if tokenEnd < input.len:
    let rest = input[tokenEnd .. ^1]
    let argColor = if commandError(input, workspace).len > 0: t.error else: t.warning
    result.add t.paint(argColor, rest)

proc highlightMentions(input: string): string =
  let t = currentTheme
  var i = 0
  for at, tokEnd in mentionTokens(input):
    result.add input[i ..< at]
    result.add t.paint(t.boldAccent, input[at ..< tokEnd])
    i = tokEnd
  result.add input[i .. ^1]

proc highlightComposerLine*(input: string, workspace = getCurrentDir()): string =
  if input.strip.startsWith("/"):
    highlightSlashCommand(input, workspace)
  else:
    highlightMentions(input)

type
  ToolCard* = object
    id*: string
    name*: string
    summary*: string
    output*: string
    hunk*: seq[string]  ## display-only edit/write lines; painted on success
    cachedLines: seq[string]  ## split once when output arrives
    isError*: bool
    pending*: bool
    expanded*: bool

  ScrollKind = enum
    skLine
    skTool

  ScrollItem = object
    case kind: ScrollKind
    of skLine:
      text: string
    of skTool:
      tool: ToolCard

  TUI* = object
    scrollback: seq[ScrollItem]
    scrollOffset: int
    input: string
    cursor: int
    history: seq[string]
    historyIdx: int
    suggestionIndex: int
    workspace: string
    sessionDir: string
    modelPicker*: ModelPicker
    footerText: string
    busy: bool
    shouldExit: bool
    interrupted: bool
    width: int
    height: int
    initialized: bool
    scrollbackDirty: bool
    streamActive: bool
    streamText: string
    streamCache: seq[string]
    visualWidth: int
    visualValid: bool
    layoutValid: bool
    streamValid: bool
    streamLineStart: int
    streamTailRows: int
    streamWrapWidth: int
    dirtyVisRow: int  ## single visual row to repaint; -1 = none
    sugCache: seq[string]
    sugKey: string
    selActive: bool
    selDragging: bool
    selDragged: bool
    selARow, selACol: int
    selBRow, selBCol: int
    layoutSpans: seq[LayoutSpan]
    layoutRows: int
    spinnerAt: int

  LayoutSpan = object
    itemIdx: int
    height: int
    lineHeights: seq[int]
    sources: seq[string]
    wrapped: seq[seq[string]]

proc invalidateVisual(tui: var TUI) =
  tui.layoutValid = false
  tui.streamValid = false
  tui.visualValid = false
  tui.scrollbackDirty = true

proc invalidateStream(tui: var TUI) =
  tui.streamValid = false
  tui.visualValid = false
  tui.scrollbackDirty = true

const
  toolPreviewLines = 2
  toolExpandMaxLines = 1000  ## source cap; paint wraps only the visible slice.

proc splitOutput(output: string): seq[string] =
  if output.len == 0: return
  result = output.splitLines
  while result.len > 0 and result[^1].len == 0:
    result.setLen(result.len - 1)

proc cardLines(card: ToolCard): seq[string] =
  if not card.isError and not card.pending and card.hunk.len > 0:
    return card.hunk
  if card.cachedLines.len > 0: card.cachedLines
  else: splitOutput(card.output)

proc skipMeta(lines: seq[string], name: string): int =
  ## Payload start for collapsed preview (headers already live in the title).
  case name
  of "bash":
    while result < lines.len:
      let s = lines[result]
      if s.startsWith("exit_code:") or s.startsWith("duration_ms:") or
         s.len == 0 or s == "stdout:" or s == "stderr:":
        inc result
      else:
        break
  of "read":
    while result < lines.len and lines[result].len > 0:
      inc result
    if result < lines.len and lines[result].len == 0:
      inc result
  else:
    discard

proc hunkPreview(lines: seq[string], n: int): seq[string] =
  var minus = ""
  var pluses: seq[string]
  for line in lines:
    let p = stripAnsi(line)
    if p.startsWith("- ") and minus.len == 0:
      minus = line
    elif p.startsWith("+ "):
      pluses.add line
  if minus.len > 0:
    result.add minus
  let room = n - result.len
  for i, p in pluses:
    if i >= room: break
    result.add p
  if result.len == 0:
    result = lines[0 ..< min(n, lines.len)]

proc previewLines(card: ToolCard, n: int): seq[string] =
  let lines = cardLines(card)
  if lines.len == 0: return
  if card.isError:
    return if lines.len <= n: lines else: lines[^n .. ^1]
  if card.hunk.len > 0:
    return hunkPreview(lines, n)
  let start = skipMeta(lines, card.name)
  let payload = if start < lines.len: lines[start .. ^1] else: @[]
  if payload.len == 0:
    return lines[0 ..< min(n, lines.len)]
  payload[0 ..< min(n, payload.len)]

proc hunkStatHint(hunk: seq[string]): string =
  var plus, minus = 0
  for line in hunk:
    let p = stripAnsi(line)
    if p.startsWith("+ "): inc plus
    elif p.startsWith("- "): inc minus
  let t = currentTheme
  if plus == 0 and minus == 0: return ""
  result = "  " & t.success & "+" & $plus
  if minus > 0:
    result.add t.muted & " " & t.error & "-" & $minus

proc isExpandable*(card: ToolCard): bool =
  if card.pending: return false
  cardLines(card).len > previewLines(card, toolPreviewLines).len

proc toolCardLines*(card: ToolCard): seq[string] =
  ## Collapsed summary by default; `expanded` shows the full output.
  let t = currentTheme
  let rail = toolRail(t, card.isError)
  let title =
    if card.isError: t.error & "● " & card.name
    else: t.accent & "● " & card.name
  let lines = cardLines(card)
  let showHunk = not card.isError and card.hunk.len > 0
  let preview = previewLines(card, toolPreviewLines)
  let expandable = not card.pending and lines.len > preview.len
  var hint = ""
  if card.pending:
    hint = "  …"
  elif card.expanded and expandable:
    hint = "  ▾"
  elif expandable:
    hint = "  ▸ " & $(lines.len - preview.len)
  var stats = ""
  if showHunk:
    stats = hunkStatHint(card.hunk)
  result.add rail & title & t.muted & "  " & card.summary & stats & t.muted & hint
  if card.pending:
    if card.name == "think":
      for line in preview:
        result.add rail & t.dim & line
    else:
      result.add rail & t.dim & "working"
    return
  proc paintLine(line: string): string =
    if showHunk: rail & line
    elif card.isError: rail & t.error & line
    else: rail & t.dim & line
  if card.expanded:
    let shown = min(lines.len, toolExpandMaxLines)
    for i in 0 ..< shown:
      result.add paintLine(lines[i])
    if lines.len > shown:
      result.add rail & t.dim & "↳ … " & $(lines.len - shown) & " more truncated"
  else:
    for line in preview:
      result.add paintLine(line)

proc itemSources(item: ScrollItem): seq[string] =
  case item.kind
  of skLine:
    result = @[item.text]
  of skTool:
    result = toolCardLines(item.tool)
    result.add ""

proc itemLayout(item: ScrollItem, w: int):
    tuple[h: int, heights: seq[int], sources: seq[string],
          wrapped: seq[seq[string]]] =
  result.sources = itemSources(item)
  for s in result.sources:
    let rows = wrapLineForWidth(s, w)
    result.wrapped.add rows
    result.heights.add rows.len
    result.h += rows.len

proc ensureLayout(tui: var TUI) =
  if tui.layoutValid: return
  let w = max(1, tui.width)
  tui.layoutSpans.setLen(0)
  tui.layoutRows = 0
  for i, item in tui.scrollback.pairs:
    let lay = itemLayout(item, w)
    tui.layoutSpans.add LayoutSpan(itemIdx: i, height: lay.h,
      lineHeights: lay.heights, sources: lay.sources, wrapped: lay.wrapped)
    tui.layoutRows += lay.h
  tui.layoutValid = true

proc visualLen(tui: TUI): int =
  tui.layoutRows + tui.streamCache.len

proc visualAt(tui: TUI, i: int): string =
  if i < 0: return
  if i >= tui.layoutRows:
    let j = i - tui.layoutRows
    if j >= 0 and j < tui.streamCache.len: return tui.streamCache[j]
    return
  var acc = 0
  for span in tui.layoutSpans:
    if i < acc + span.height:
      var inner = 0
      let local = i - acc
      for j, h in span.lineHeights:
        if local < inner + h:
          if j < 0 or j >= span.wrapped.len: return
          let idx = local - inner
          if idx >= 0 and idx < span.wrapped[j].len: return span.wrapped[j][idx]
          return
        inner += h
      return
    acc += span.height

proc ownerAt(tui: TUI, visRow: int): int =
  if visRow < 0 or visRow >= tui.layoutRows: return -1
  var acc = 0
  for span in tui.layoutSpans:
    if visRow < acc + span.height:
      let local = visRow - acc
      if local == span.height - 1 and
          tui.scrollback[span.itemIdx].kind == skTool:
        return -1
      return span.itemIdx
    acc += span.height
  -1

proc growPlainWrap*(cache: var seq[string], text: string,
                    lineStart, tailRows: var int, width: int) =
  ## Append-wrap `text` into `cache`. Complete lines stay; only the
  ## unfinished tail is rewrapped. `lineStart`/`tailRows` are the cursor.
  let w = max(1, width)
  if tailRows > 0:
    if tailRows <= cache.len:
      cache.setLen(cache.len - tailRows)
    tailRows = 0
  var i = lineStart
  while i < text.len:
    var j = i
    while j < text.len and text[j] != '\n':
      inc j
    let wrapped = wrapRunes(text[i ..< j], w)
    if j < text.len:
      cache.add wrapped
      i = j + 1
      lineStart = i
    else:
      cache.add wrapped
      tailRows = wrapped.len
      lineStart = i
      return
  tailRows = 0

proc wrapStream(tui: var TUI) =
  ## Live stream is plain wrap. Markdown runs once in finishStream.
  let w = max(1, tui.width)
  if not tui.streamActive:
    tui.streamCache.setLen(0)
    tui.streamLineStart = 0
    tui.streamTailRows = 0
    tui.streamWrapWidth = w
    tui.streamValid = true
    return
  if tui.streamText.len == 0:
    tui.streamCache.setLen(0)
    tui.streamCache.add "…"
    tui.streamLineStart = 0
    tui.streamTailRows = 1
    tui.streamWrapWidth = w
    tui.streamValid = true
    return
  if tui.streamWrapWidth != w:
    tui.streamCache.setLen(0)
    tui.streamLineStart = 0
    tui.streamTailRows = 0
    tui.streamWrapWidth = w
  elif tui.streamCache.len == 1 and tui.streamCache[0] == "…" and
      tui.streamLineStart == 0:
    tui.streamCache.setLen(0)
    tui.streamTailRows = 0
  growPlainWrap(tui.streamCache, tui.streamText, tui.streamLineStart,
    tui.streamTailRows, w)
  tui.streamValid = true

proc ensureVisual(tui: var TUI) =
  if tui.visualWidth != tui.width:
    tui.layoutValid = false
    tui.streamValid = false
    tui.visualValid = false
  if tui.visualValid:
    return
  if not tui.layoutValid:
    tui.ensureLayout()
  if not tui.streamValid:
    tui.wrapStream()
  tui.visualWidth = tui.width
  tui.visualValid = true

proc clearSelection(tui: var TUI) =
  if tui.selActive or tui.selDragging:
    tui.selActive = false
    tui.selDragging = false
    tui.selDragged = false
    tui.scrollbackDirty = true

proc normalizeSelection(tui: TUI): tuple[r1, c1, r2, c2: int] =
  var r1 = tui.selARow
  var c1 = tui.selACol
  var r2 = tui.selBRow
  var c2 = tui.selBCol
  if r1 > r2 or (r1 == r2 and c1 > c2):
    swap(r1, r2)
    swap(c1, c2)
  (r1, c1, r2, c2)

proc selectionText(tui: var TUI): string =
  tui.ensureVisual()
  if not tui.selActive or not tui.selDragged: return ""
  let (r1, c1, r2, c2) = tui.normalizeSelection()
  let n = tui.visualLen
  if r1 < 0 or r1 >= n: return ""
  var parts: seq[string] = @[]
  for row in r1 .. min(r2, n - 1):
    let plain = stripAnsi(tui.visualAt(row))
    let a = if row == r1: clamp(c1, 0, plain.runeLen) else: 0
    let b = if row == r2: clamp(c2 + 1, 0, plain.runeLen) else: plain.runeLen
    if b > a:
      parts.add plain.runeSubStr(a, b - a)
    else:
      parts.add ""
  parts.join("\n")

proc screenToVisual(tui: var TUI, x, y, scrollbackHeight: int): tuple[ok: bool, row, col: int] =
  if y < 0 or y >= scrollbackHeight: return (false, 0, 0)
  tui.ensureVisual()
  let n = tui.visualLen
  let endIdx = max(-1, n - 1 - tui.scrollOffset)
  let startIdx = max(0, endIdx - scrollbackHeight + 1)
  let row = startIdx + y
  if row < 0 or row >= n: return (false, 0, 0)
  let plain = stripAnsi(tui.visualAt(row))
  (true, row, clamp(x, 0, max(0, plain.runeLen - 1)))

const composerHistoryLimit = 500

proc loadComposerHistory*(path: string): seq[string] =
  if path.len == 0 or not fileExists(path): return
  for line in lines(path):
    let stripped = line.strip
    if stripped.len == 0: continue
    try:
      result.add parseJson(stripped).getStr
    except CatchableError:
      discard

proc saveComposerHistory*(entries: seq[string], path: string) =
  if path.len == 0: return
  let start = max(0, entries.len - composerHistoryLimit)
  let parent = path.parentDir
  if parent.len > 0: createDir(parent)
  var body = ""
  for i in start ..< entries.len:
    body.add $(%entries[i])
    body.add "\n"
  writeFile(path, body)

proc insertAt*(s: string, pos: int, piece: string): tuple[text: string, cursor: int] =
  let p = clamp(pos, 0, s.len)
  (s[0 ..< p] & piece & s[p .. ^1], p + piece.len)

proc deleteBefore*(s: string, pos: int): tuple[text: string, cursor: int] =
  if pos <= 0: return (s, 0)
  var i = pos - 1
  while i > 0 and (uint8(s[i]) and 0xC0) == 0x80:
    dec i
  (s[0 ..< i] & s[pos .. ^1], i)

proc deleteAfter*(s: string, pos: int): tuple[text: string, cursor: int] =
  if pos >= s.len: return (s, pos)
  var i = pos
  var r: Rune
  fastRuneAt(s, i, r)
  (s[0 ..< pos] & s[i .. ^1], pos)

proc composerView*(input: string, cursor, innerWidth: int):
    tuple[lines: seq[string], row, col: int] =
  let inner = max(1, innerWidth)
  let cur = clamp(cursor, 0, input.len)
  var found = false
  var i = 0
  while true:
    let lineStart = i
    while i < input.len and input[i] != '\n':
      inc i
    let logical = input[lineStart ..< i]
    let wrapped = wrapRunes(logical, inner)
    var bytePos = lineStart
    for wline in wrapped:
      if not found and cur >= bytePos and cur <= bytePos + wline.len:
        var c = 0
        var b = bytePos
        while b < cur and b < bytePos + wline.len:
          var r: Rune
          fastRuneAt(input, b, r)
          inc c
        result.row = result.lines.len
        result.col = c
        found = true
      result.lines.add wline
      bytePos += wline.len
    if i < input.len and input[i] == '\n':
      inc i
      if i >= input.len:
        result.lines.add ""
        if not found and cur >= i:
          result.row = result.lines.high
          result.col = 0
          found = true
        break
      continue
    break
  if result.lines.len == 0:
    result.lines.add ""
  if not found:
    result.row = result.lines.high
    result.col = runeLen(result.lines[^1])

proc visualToCursor*(input: string, innerWidth, row, col: int): int =
  let inner = max(1, innerWidth)
  var vrow = 0
  var i = 0
  while true:
    let lineStart = i
    while i < input.len and input[i] != '\n':
      inc i
    let logical = input[lineStart ..< i]
    let wrapped = wrapRunes(logical, inner)
    var bytePos = lineStart
    for wline in wrapped:
      if vrow == row:
        var c = 0
        var b = bytePos
        let endB = bytePos + wline.len
        while c < col and b < endB:
          var r: Rune
          fastRuneAt(input, b, r)
          inc c
        return b
      bytePos += wline.len
      inc vrow
    if i < input.len and input[i] == '\n':
      inc i
      if i >= input.len:
        if vrow == row: return input.len
        break
      continue
    break
  input.len

proc initTUI*(workspace = getCurrentDir(), sessionDir = ""): TUI =
  termInit()
  result.width = termWidth()
  result.height = termHeight()
  result.historyIdx = -1
  result.suggestionIndex = -1
  result.workspace = expandFilename(workspace)
  result.sessionDir = sessionDir
  result.history = loadComposerHistory(niminalConfigDir() / "history")
  result.initialized = true
  result.scrollbackDirty = true
  result.visualValid = false
  result.layoutValid = false
  result.streamValid = false
  result.dirtyVisRow = -1

proc shutdown*(tui: var TUI) =
  if tui.initialized:
    termShutdown()
    tui.initialized = false

const
  maxInputRows = 8
  statusHints = "esc clears · @ file · ^v clip · click tool · drag copy · wheel/pgup scroll"
  spinnerFrames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]

proc composerInner(tui: TUI): int =
  max(1, tui.width - 2)

proc inputRows(tui: TUI): int =
  let n = composerView(tui.input, tui.cursor, tui.composerInner).lines.len
  min(max(1, n), maxInputRows)

proc resetSuggestion(tui: var TUI) =
  tui.suggestionIndex = -1

proc suggestionStep*(index, delta, count: int): int =
  ## First arrow (or a fresh list) lands on 0. No wrap — the top match
  ## stays selected instead of jumping to the last row.
  if count <= 0: return -1
  if index < 0: return 0
  clamp(index + delta, 0, count - 1)

proc slashSuggestions(tui: var TUI): seq[string] =
  let key = tui.input & "\0" & $tui.cursor
  if key == tui.sugKey:
    return tui.sugCache
  tui.sugCache = commandSuggestions(tui.input, tui.workspace, tui.sessionDir,
    tui.modelPicker, tui.cursor)
  tui.sugKey = key
  tui.suggestionIndex = if tui.sugCache.len > 0: 0 else: -1
  tui.sugCache

proc moveSuggestion(tui: var TUI, delta: int): bool =
  let suggestions = tui.slashSuggestions
  if suggestions.len == 0:
    tui.resetSuggestion()
    return false
  let next = suggestionStep(tui.suggestionIndex, delta, suggestions.len)
  if next == tui.suggestionIndex:
    return tui.suggestionIndex >= 0
  tui.suggestionIndex = next
  tui.scrollbackDirty = true
  true

proc acceptSuggestion(tui: var TUI): bool =
  let suggestions = tui.slashSuggestions
  if suggestions.len == 0:
    return false
  let idx = if tui.suggestionIndex < 0: 0 else: tui.suggestionIndex
  if idx >= suggestions.len:
    return false
  let choice = suggestions[idx]
  if choice.startsWith("@") and mentionAt(tui.input, tui.cursor).active:
    let ins = applyMention(tui.input, tui.cursor, choice)
    tui.input = ins.text
    tui.cursor = ins.cursor
    tui.resetSuggestion()
    tui.scrollbackDirty = true
    return false
  tui.input = choice
  tui.cursor = tui.input.len
  tui.resetSuggestion()
  tui.scrollbackDirty = true
  true

proc commandMenuRows(tui: var TUI): int =
  let suggestions = tui.slashSuggestions
  if suggestions.len == 0:
    return 0
  min(suggestions.len, min(10, max(0, tui.height - tui.inputRows - 3)))

proc chromeRows(tui: var TUI): int =
  1 + tui.commandMenuRows + tui.inputRows + 1

proc contentHeight(tui: var TUI): int =
  max(1, tui.height - tui.chromeRows)

proc maxScroll*(tui: var TUI): int =
  tui.ensureVisual()
  max(0, tui.visualLen - tui.contentHeight)

proc scrollBy(tui: var TUI, delta: int) =
  let prev = tui.scrollOffset
  tui.scrollOffset = clamp(tui.scrollOffset + delta, 0, tui.maxScroll)
  if tui.scrollOffset != prev:
    tui.scrollbackDirty = true

proc applyScrollKey(tui: var TUI, key: Key): bool =
  case key
  of keyPageUp:
    tui.scrollBy(tui.contentHeight)
    true
  of keyPageDown:
    tui.scrollBy(-tui.contentHeight)
    true
  of keyCtrlB:
    tui.scrollBy(max(1, tui.contentHeight div 2))
    true
  of keyCtrlF:
    tui.scrollBy(-max(1, tui.contentHeight div 2))
    true
  else:
    false

proc updateSelectionEnd(tui: var TUI, x, y, scrollbackHeight: int) =
  let hit = tui.screenToVisual(x, y, scrollbackHeight)
  if hit.ok:
    if hit.row != tui.selARow or hit.col != tui.selACol:
      tui.selDragged = true
    tui.selBRow = hit.row
    tui.selBCol = hit.col
    tui.scrollbackDirty = true

proc spliceToolWrap(tui: var TUI, idx: int) =
  ## Rewrap one tool card; leave the rest of the layout alone.
  if not tui.layoutValid or tui.width <= 0:
    tui.invalidateVisual()
    return
  let lay = itemLayout(tui.scrollback[idx], max(1, tui.width))
  var acc = 0
  for s in tui.layoutSpans.mitems:
    if s.itemIdx == idx:
      let old = s.height
      tui.layoutRows += lay.h - old
      s.height = lay.h
      s.lineHeights = lay.heights
      s.sources = lay.sources
      s.wrapped = lay.wrapped
      if lay.h == old:
        tui.dirtyVisRow = acc + max(0, lay.h - 1)
      else:
        tui.visualValid = false
        tui.scrollbackDirty = true
      return
    acc += s.height
  tui.invalidateVisual()

proc setToolExpanded(tui: var TUI, idx: int, expanded: bool) =
  if idx < 0 or idx > tui.scrollback.high: return
  if tui.scrollback[idx].kind != skTool: return
  var card = tui.scrollback[idx].tool
  if not isExpandable(card) or card.expanded == expanded: return
  card.expanded = expanded
  tui.scrollback[idx] = ScrollItem(kind: skTool, tool: card)
  tui.spliceToolWrap(idx)

proc toggleToolAt(tui: var TUI, visRow: int): bool =
  tui.ensureVisual()
  let idx = tui.ownerAt(visRow)
  if idx < 0 or tui.scrollback[idx].kind != skTool: return false
  if not isExpandable(tui.scrollback[idx].tool): return false
  tui.setToolExpanded(idx, not tui.scrollback[idx].tool.expanded)
  true

proc toggleAllTools(tui: var TUI) =
  var expand = false
  var any = false
  for item in tui.scrollback:
    if item.kind == skTool and isExpandable(item.tool):
      any = true
      if not item.tool.expanded:
        expand = true
        break
  if not any: return
  for i in 0 .. tui.scrollback.high:
    if tui.scrollback[i].kind != skTool: continue
    var card = tui.scrollback[i].tool
    if isExpandable(card) and card.expanded != expand:
      card.expanded = expand
      tui.scrollback[i] = ScrollItem(kind: skTool, tool: card)
  tui.invalidateVisual()

proc handleMouse(tui: var TUI, ev: InputEvent) =
  let scrollbackHeight = tui.contentHeight
  case ev.mouse
  of mousePress:
    let hit = tui.screenToVisual(ev.mouseX, ev.mouseY, scrollbackHeight)
    if hit.ok:
      tui.selActive = true
      tui.selDragging = true
      tui.selDragged = false
      tui.selARow = hit.row
      tui.selACol = hit.col
      tui.selBRow = hit.row
      tui.selBCol = hit.col
      tui.scrollbackDirty = true
    else:
      tui.clearSelection()
  of mouseDrag:
    if not tui.selDragging: return
    tui.updateSelectionEnd(ev.mouseX, ev.mouseY, scrollbackHeight)
  of mouseRelease:
    if not tui.selDragging: return
    tui.selDragging = false
    tui.updateSelectionEnd(ev.mouseX, ev.mouseY, scrollbackHeight)
    if tui.selDragged:
      let text = tui.selectionText()
      if text.len > 0:
        copyToClipboard(text)
    else:
      discard tui.toggleToolAt(tui.selARow)
      tui.clearSelection()
    tui.scrollbackDirty = true
  of mouseNone:
    discard

proc addLine*(tui: var TUI, line: string) =
  for l in line.splitLines:
    tui.scrollback.add ScrollItem(kind: skLine, text: l)
  tui.scrollOffset = 0
  tui.invalidateVisual()

proc addLines*(tui: var TUI, lines: openArray[string]) =
  for l in lines:
    tui.addLine(l)

proc addUserMessage*(tui: var TUI, text: string) =
  for line in userCardLines(text):
    tui.scrollback.add ScrollItem(kind: skLine, text: line)
  tui.scrollOffset = 0
  tui.invalidateVisual()

proc toolCallSummary(call: ContentBlock): string =
  if call.input.isNil: return ""
  case call.name
  of "read":
    let path = call.input.getOrDefault("path").getStr
    let startLine = call.input.getOrDefault("start_line").getInt
    let endLine = call.input.getOrDefault("end_line").getInt
    if startLine > 0 and endLine > 0:
      path & ":" & $startLine & "-" & $endLine
    elif startLine > 0:
      path & ":" & $startLine
    else:
      path
  of "edit", "write":
    call.input.getOrDefault("path").getStr
  of "glob":
    call.input.getOrDefault("pattern").getStr
  of "grep":
    var s = call.input.getOrDefault("pattern").getStr
    let g = call.input.getOrDefault("glob").getStr
    if g.len > 0: s.add "  " & g
    let p = call.input.getOrDefault("path").getStr
    if p.len > 0: s.add "  " & p
    s
  of "bash":
    let cmd = call.input.getOrDefault("command").getStr
    if cmd.len > 80: cmd[0 ..< 77] & "..." else: cmd
  else:
    call.input.pretty(0)

proc makeToolCard(call: ContentBlock, pending = true): ToolCard =
  result = ToolCard(id: call.id, name: call.name, summary: toolCallSummary(call),
                    pending: pending)
  if call.kind == ckToolUse:
    result.hunk = formatToolHunk(call.name, call.input, true)

proc addToolStart*(tui: var TUI, call: ContentBlock) =
  tui.scrollback.add ScrollItem(kind: skTool, tool: makeToolCard(call))
  tui.scrollOffset = 0
  tui.invalidateVisual()

proc finishToolCard(items: var seq[ScrollItem], id, output: string, isError: bool) =
  var found = -1
  for i, item in items.pairs:
    if item.kind != skTool or not item.tool.pending: continue
    if id.len > 0 and item.tool.id.len > 0:
      if item.tool.id == id: found = i
    else:
      found = i
  var card = if found >= 0: items[found].tool
             else: ToolCard(name: "tool")
  card.output = output
  card.cachedLines = splitOutput(output)
  card.isError = isError
  card.pending = false
  if found >= 0:
    items[found] = ScrollItem(kind: skTool, tool: card)
  else:
    items.add ScrollItem(kind: skTool, tool: card)

proc addToolResult*(tui: var TUI, output: string, isError: bool) =
  tui.scrollback.finishToolCard("", output, isError)
  tui.scrollOffset = 0
  tui.invalidateVisual()

proc thinkCardIndex(tui: TUI): int =
  for i in countdown(tui.scrollback.high, 0):
    if tui.scrollback[i].kind == skTool and tui.scrollback[i].tool.name == "think":
      return i
  -1

proc appendThinking*(tui: var TUI, delta: string) =
  if delta.len == 0: return
  var idx = tui.thinkCardIndex
  if idx < 0 or not tui.scrollback[idx].tool.pending:
    tui.scrollback.add ScrollItem(kind: skTool,
      tool: ToolCard(name: "think", pending: true))
    idx = tui.scrollback.high
  var card = tui.scrollback[idx].tool
  card.output.add delta
  if card.cachedLines.len == 0 or '\n' in delta:
    card.cachedLines = splitOutput(card.output)
  else:
    card.cachedLines[^1].add delta
  tui.scrollback[idx] = ScrollItem(kind: skTool, tool: card)
  tui.scrollOffset = 0
  tui.spliceToolWrap(idx)

proc finishThinking*(tui: var TUI) =
  let idx = tui.thinkCardIndex
  if idx < 0: return
  var card = tui.scrollback[idx].tool
  if not card.pending: return
  card.pending = false
  tui.scrollback[idx] = ScrollItem(kind: skTool, tool: card)
  tui.spliceToolWrap(idx)

proc addItemLine(items: var seq[ScrollItem], text: string) =
  items.add ScrollItem(kind: skLine, text: text)

const markdownResumeTail = 40

proc assistantTextCount(session: Session): int =
  for event in session.events:
    if event.kind != sekAssistant: continue
    for b in event.message.content:
      if b.kind == ckText and b.text.len > 0: inc result

proc addAssistantVisual(items: var seq[ScrollItem], text: string, markdown: bool) =
  if markdown:
    for line in renderMarkdown(text, true).splitLines:
      items.addItemLine(line)
  else:
    for line in text.splitLines:
      items.addItemLine(line)
  items.addItemLine("")

proc transcriptItems(session: Session): seq[ScrollItem] =
  ## Compact visual history: user cards, tool cards, rendered assistant text.
  ## ponytail: only the last 40 assistant texts get markdown on resume;
  ## older stay plain so huge transcripts don't hitch at load.
  let mdFrom = max(0, assistantTextCount(session) - markdownResumeTail)
  var textIdx = 0
  for i in 0 ..< session.events.len:
    let event = session.events[i]
    case event.kind
    of sekUser:
      var body = ""
      for b in event.message.content:
        case b.kind
        of ckText:
          if body.len > 0: body.add "\n"
          body.add b.text
        of ckImage:
          if body.len > 0: body.add "\n"
          body.add "[" & b.mimeType & "]"
        else:
          discard
      for line in userCardLines(body):
        result.addItemLine(line)
    of sekAssistant:
      for b in event.message.content:
        case b.kind
        of ckText:
          if b.text.len == 0: continue
          result.addAssistantVisual(b.text, textIdx >= mdFrom)
          inc textIdx
        of ckToolUse:
          result.add ScrollItem(kind: skTool, tool: makeToolCard(b))
        of ckThinking:
          if b.thinking.len > 0:
            result.add ScrollItem(kind: skTool, tool: ToolCard(
              name: "think", output: b.thinking,
              cachedLines: splitOutput(b.thinking)))
        of ckToolResult:
          discard
        of ckImage:
          discard
    of sekToolResult:
      result.finishToolCard(event.toolId, event.toolOutput, event.toolError)
    of sekCompaction:
      result.addItemLine(currentTheme.paint(currentTheme.dim, "Context compacted"))
      result.addItemLine("")
    of sekName:
      discard

proc transcriptLines*(session: Session): seq[string] =
  for item in transcriptItems(session):
    case item.kind
    of skLine:
      result.add item.text
    of skTool:
      result.add toolCardLines(item.tool)
      result.add ""

proc replaySession*(tui: var TUI, session: Session) =
  if session.events.len == 0: return
  for item in transcriptItems(session):
    tui.scrollback.add item
  tui.scrollOffset = 0
  tui.invalidateVisual()

proc addAssistantMarkdown*(tui: var TUI, text: string) =
  if text.len == 0: return
  for line in renderMarkdown(text, true).splitLines:
    tui.scrollback.add ScrollItem(kind: skLine, text: line)
  tui.scrollback.add ScrollItem(kind: skLine, text: "")
  tui.scrollOffset = 0
  tui.invalidateVisual()

proc beginStream*(tui: var TUI) =
  tui.streamActive = true
  tui.streamText = ""
  tui.streamCache.setLen(0)
  tui.streamLineStart = 0
  tui.streamTailRows = 0
  tui.scrollOffset = 0
  tui.invalidateStream()

proc appendStream*(tui: var TUI, delta: string) =
  if not tui.streamActive or delta.len == 0: return
  tui.streamText.add delta
  tui.streamValid = false
  tui.visualValid = false

proc finishStream*(tui: var TUI, response: ProviderResponse) =
  tui.streamActive = false
  tui.streamText = ""
  tui.addAssistantMarkdown(response.textContent())

proc resetStream(tui: var TUI) =
  if not tui.streamActive: return
  tui.streamActive = false
  tui.streamText = ""
  tui.streamCache.setLen(0)
  tui.streamLineStart = 0
  tui.streamTailRows = 0
  tui.invalidateVisual()

proc clearScrollback(tui: var TUI) =
  tui.scrollback.setLen(0)
  tui.scrollOffset = 0
  tui.clearSelection()
  tui.resetStream()
  tui.invalidateVisual()

proc loadSessionView*(tui: var TUI, session: Session) =
  ## Replace the transcript when switching sessions (`/resume`, `/new`).
  tui.clearScrollback()
  let t = currentTheme
  if session.events.len == 0:
    tui.addLine(t.paint(t.dim, "New session #" & session.id))
    tui.addLine("")
  else:
    tui.addLine(t.paint(t.dim, "Resumed #" & session.id))
    tui.addLine("")
    tui.replaySession(session)

proc cancelStream*(tui: var TUI) =
  tui.finishThinking()
  tui.resetStream()
  tui.addLine(currentTheme.paint(currentTheme.warning, "Interrupted"))
  tui.addLine("")

proc discardStream*(tui: var TUI) =
  tui.resetStream()

proc busySpinner(tui: TUI): string =
  let t = currentTheme
  t.paint(t.dim, spinnerFrames[tui.spinnerAt mod spinnerFrames.len] & " working")

proc setBusy*(tui: var TUI, busy: bool) =
  tui.busy = busy
  if busy:
    tui.interrupted = false
    tui.spinnerAt = 0
  tui.scrollbackDirty = true

proc setFooter*(tui: var TUI, text: string) =
  tui.footerText = text

proc shouldExit*(tui: TUI): bool =
  tui.shouldExit

proc wasInterrupted*(tui: TUI): bool =
  tui.interrupted

proc statusLine(tui: var TUI, w: int): string =
  let t = currentTheme
  var parts: seq[string] = @[]
  if tui.scrollOffset > 0:
    parts.add t.paint(t.accent, "↑" & $tui.scrollOffset)
  if tui.footerText.len > 0:
    parts.add tui.footerText
  if not tui.busy:
    let error = commandError(tui.input, tui.workspace)
    let suggestions = tui.slashSuggestions
    if error.len > 0:
      parts.add t.paint(t.error, error)
    elif suggestions.len == 0:
      parts.add t.paint(t.dim, statusHints)
  let joined = parts.join("  ")
  let visible = stripAnsi(joined)
  if runeLen(visible) > w:
    truncateAnsi(joined, w)
  else:
    joined

proc paintContentRow(tui: var TUI, visIdx, screenRow: int) =
  setCursor(0, screenRow)
  let line = tui.visualAt(visIdx)
  if tui.selActive and tui.selDragged:
    let (sr1, sc1, sr2, sc2) = tui.normalizeSelection()
    if visIdx >= sr1 and visIdx <= sr2:
      let plainLen = stripAnsi(line).runeLen
      let a = if visIdx == sr1: clamp(sc1, 0, plainLen) else: 0
      let b = if visIdx == sr2: clamp(sc2 + 1, 0, plainLen) else: plainLen
      writeLineWithSelection(line, a, b)
      stdout.write("\e[K\e[0m")
      return
  stdout.write(line)
  stdout.write("\e[K\e[0m")

proc render*(tui: var TUI) =
  let w = termWidth()
  let h = termHeight()
  let resized = w != tui.width or h != tui.height
  if resized:
    tui.width = w
    tui.height = h
    clearScreen()
    tui.visualValid = false
    tui.layoutValid = false
    tui.streamValid = false
    tui.scrollbackDirty = true
    tui.scrollOffset = min(tui.scrollOffset, tui.maxScroll)

  if h < 4: return
  if tui.busy:
    inc tui.spinnerAt

  let view = composerView(tui.input, tui.cursor, tui.composerInner)
  let inRows = min(max(1, view.lines.len), maxInputRows)
  let suggestions = if tui.busy: @[] else: tui.slashSuggestions
  let menuRows =
    if suggestions.len == 0: 0
    else: min(suggestions.len, min(10, max(0, h - inRows - 3)))
  let scrollbackHeight = max(1, h - (1 + menuRows + inRows + 1))
  let sepRow = scrollbackHeight
  let menuTop = sepRow + 1
  let inputTop = menuTop + menuRows
  let statusRow = h - 1

  let prevStreamRows = tui.streamCache.len
  tui.ensureVisual()
  let n = tui.visualLen
  let endIdx = max(-1, n - 1 - tui.scrollOffset)
  let startIdx = max(0, endIdx - scrollbackHeight + 1)
  let streamGrew = tui.streamCache.len != prevStreamRows
  let fullContent = tui.scrollbackDirty or streamGrew or
    (tui.selActive and tui.selDragged)

  if fullContent:
    var row = 0
    if endIdx >= 0:
      for i in startIdx .. endIdx:
        if row >= scrollbackHeight: break
        tui.paintContentRow(i, row)
        inc row
    while row < scrollbackHeight:
      setCursor(0, row)
      stdout.write("\e[K")
      inc row
    tui.scrollbackDirty = false
    tui.dirtyVisRow = -1
  else:
    var visIdx = tui.dirtyVisRow
    if visIdx < 0 and tui.streamActive and tui.streamCache.len > 0:
      visIdx = n - 1
    if visIdx >= startIdx and visIdx <= endIdx:
      tui.paintContentRow(visIdx, visIdx - startIdx)
    tui.dirtyVisRow = -1

  setCursor(0, sepRow)
  let t = currentTheme
  stdout.write(t.dim & "─".repeat(w) & t.reset & "\e[K")

  let menuLines = formatCommandMenu(suggestions, tui.suggestionIndex, menuRows, w,
    tui.workspace, tui.sessionDir)
  for r in 0 ..< menuRows:
    setCursor(0, menuTop + r)
    if r < menuLines.len:
      stdout.write(menuLines[r])
    else:
      stdout.write(t.panelBg)
    stdout.write("\e[K" & t.reset)

  let firstVisible = max(0, min(view.row, max(0, view.lines.len - inRows)))
  let firstLogical = tui.input.split('\n', maxsplit = 1)[0]
  let highlightFirst = wrapRunes(firstLogical, tui.composerInner).len == 1
  for r in 0 ..< inRows:
    setCursor(0, inputTop + r)
    let lineIdx = firstVisible + r
    if tui.busy and r == 0:
      stdout.write(tui.busySpinner() & "\e[K" & t.reset)
    elif lineIdx < view.lines.len:
      let body = view.lines[lineIdx]
      if menuRows > 0:
        stdout.write(userRail() & body & "\e[K" & t.reset)
      else:
        let prefix = if lineIdx == 0: t.accent & "> " & t.reset else: "  "
        let displayed =
          if lineIdx == 0 and highlightFirst:
            highlightComposerLine(body, tui.workspace)
          else: body
        stdout.write(prefix & displayed & "\e[K" & t.reset)
    else:
      stdout.write("\e[K")

  setCursor(0, statusRow)
  stdout.write(tui.statusLine(w) & "\e[K")

  if tui.busy:
    hideCursor()
  else:
    showCursor()
    let visRow = clamp(view.row - firstVisible, 0, inRows - 1)
    let cursorX = 2 + min(view.col, max(0, w - 3))
    setCursor(cursorX, inputTop + visRow)

  stdout.flushFile()

proc historyPrev(tui: var TUI) =
  if tui.history.len == 0: return
  tui.resetSuggestion()
  if tui.historyIdx == -1:
    tui.historyIdx = tui.history.high
  elif tui.historyIdx > 0:
    dec tui.historyIdx
  if tui.historyIdx >= 0 and tui.historyIdx < tui.history.len:
    tui.input = tui.history[tui.historyIdx]
    tui.cursor = tui.input.len
    tui.scrollbackDirty = true

proc historyNext(tui: var TUI) =
  if tui.historyIdx < 0: return
  tui.resetSuggestion()
  if tui.historyIdx < tui.history.high:
    inc tui.historyIdx
    tui.input = tui.history[tui.historyIdx]
    tui.cursor = tui.input.len
    tui.scrollbackDirty = true
  else:
    tui.historyIdx = -1
    tui.input = ""
    tui.cursor = 0
    tui.scrollbackDirty = true

proc rememberInput(tui: var TUI) =
  if tui.input.strip.len == 0: return
  if tui.history.len == 0 or tui.history[^1] != tui.input:
    tui.history.add tui.input
  tui.historyIdx = -1
  saveComposerHistory(tui.history, niminalConfigDir() / "history")

proc clearInput*(tui: var TUI) =
  tui.input = ""
  tui.cursor = 0
  tui.resetSuggestion()
  tui.scrollbackDirty = true

proc insertComposer(tui: var TUI, piece: string) =
  if piece.len == 0: return
  let ins = insertAt(tui.input, tui.cursor, piece)
  tui.input = ins.text
  tui.cursor = ins.cursor
  tui.resetSuggestion()
  tui.scrollbackDirty = true

proc handleEvent(tui: var TUI, ev: InputEvent): bool =
  result = false
  if ev.resized:
    tui.scrollbackDirty = true
  if ev.scrollDelta != 0:
    tui.scrollBy(ev.scrollDelta)
  if ev.mouse != mouseNone:
    tui.handleMouse(ev)
  let key = ev.key
  if tui.busy:
    if key == keyEscape or key == keyCtrlC:
      tui.interrupted = true
    elif key == keyCtrlO:
      tui.toggleAllTools()
    else:
      discard tui.applyScrollKey(key)
    return false

  case key
  of keyNone:
    discard
  of keyEscape:
    if tui.input.len > 0:
      tui.clearInput()
  of keyCtrlC:
    if tui.input.len > 0:
      tui.clearInput()
    else:
      tui.shouldExit = true
  of keyEnter:
    if resumeOpensPicker(tui.input):
      tui.input = "/resume "
      tui.cursor = tui.input.len
      tui.resetSuggestion()
      tui.scrollbackDirty = true
    elif mentionAt(tui.input, tui.cursor).active and tui.slashSuggestions.len > 0:
      discard tui.acceptSuggestion()
    elif tui.acceptSuggestion():
      tui.rememberInput()
      result = true
    elif tui.input.strip.len > 0:
      tui.rememberInput()
      result = true
  of keyShiftEnter:
    let ins = insertAt(tui.input, tui.cursor, "\n")
    tui.input = ins.text
    tui.cursor = ins.cursor
    tui.resetSuggestion()
    tui.scrollbackDirty = true
  of keyBackspace:
    let gone = deleteBefore(tui.input, tui.cursor)
    if gone.cursor != tui.cursor:
      tui.input = gone.text
      tui.cursor = gone.cursor
      tui.resetSuggestion()
      tui.scrollbackDirty = true
  of keyDelete:
    let gone = deleteAfter(tui.input, tui.cursor)
    tui.input = gone.text
    tui.resetSuggestion()
    tui.scrollbackDirty = true
  of keyLeft:
    if tui.cursor > 0:
      var i = tui.cursor - 1
      while i > 0 and (uint8(tui.input[i]) and 0xC0) == 0x80:
        dec i
      tui.cursor = i
      tui.resetSuggestion()
  of keyRight:
    if tui.cursor < tui.input.len:
      var i = tui.cursor
      var r: Rune
      fastRuneAt(tui.input, i, r)
      tui.cursor = i
      tui.resetSuggestion()
  of keyHome, keyCtrlA:
    tui.resetSuggestion()
    let view = composerView(tui.input, tui.cursor, tui.composerInner)
    tui.cursor = visualToCursor(tui.input, tui.composerInner, view.row, 0)
  of keyEnd, keyCtrlE:
    tui.resetSuggestion()
    let view = composerView(tui.input, tui.cursor, tui.composerInner)
    tui.cursor = visualToCursor(tui.input, tui.composerInner, view.row, 10_000)
  of keyUp:
    if tui.moveSuggestion(-1):
      return false
    let inner = tui.composerInner
    let view = composerView(tui.input, tui.cursor, inner)
    if view.row > 0:
      tui.cursor = visualToCursor(tui.input, inner, view.row - 1, view.col)
      tui.resetSuggestion()
    else:
      tui.historyPrev()
  of keyDown:
    if tui.moveSuggestion(1):
      return false
    let inner = tui.composerInner
    let view = composerView(tui.input, tui.cursor, inner)
    if view.row + 1 < view.lines.len:
      tui.cursor = visualToCursor(tui.input, inner, view.row + 1, view.col)
      tui.resetSuggestion()
    else:
      tui.historyNext()
  of keyCtrlP:
    tui.historyPrev()
  of keyCtrlN:
    tui.historyNext()
  of keyCtrlO:
    tui.toggleAllTools()
  of keyCtrlV:
    let raw = readClipboardImage()
    if raw.len > 0:
      let saved = saveWorkspaceImage(initWorkspace(tui.workspace), raw)
      if saved.ok:
        tui.footerText = ""
        tui.insertComposer(saved.mention & " ")
      else:
        tui.footerText = saved.err
        tui.scrollbackDirty = true
    else:
      let clip = normalizePasteText(readClipboardText())
      if clip.len > 0:
        tui.insertComposer(clip)
      else:
        tui.footerText = "clipboard has no image"
        tui.scrollbackDirty = true
  of keyPageUp, keyPageDown, keyCtrlB, keyCtrlF:
    discard tui.applyScrollKey(key)
  of keyChar:
    let piece =
      if ev.text.len > 0: ev.text
      elif ev.ch != '\0': $ev.ch
      else: ""
    if piece.len > 0:
      let mention = ingestPastedPath(initWorkspace(tui.workspace), piece)
      tui.insertComposer(if mention.len > 0: mention & " " else: piece)
  of keyTab:
    if tui.suggestionIndex < 0 and tui.slashSuggestions.len > 0:
      tui.suggestionIndex = 0
    discard tui.acceptSuggestion()

proc readLineBlocking*(tui: var TUI): string =
  tui.input = ""
  tui.cursor = 0
  tui.resetSuggestion()
  tui.scrollbackDirty = true
  tui.render()
  while not tui.shouldExit:
    let ev = readInputEvent(-1)
    if handleEvent(tui, ev):
      result = tui.input
      tui.input = ""
      tui.cursor = 0
      tui.scrollbackDirty = true
      return
    if ev.key != keyNone or ev.scrollDelta != 0 or ev.resized or ev.mouse != mouseNone:
      tui.render()
  result = ""

proc pollBusy*(tui: var TUI, waitMs = 0): bool =
  ## Non-blocking by default: drain Ctrl-C/scroll. Does not paint.
  let ev = readInputEvent(waitMs)
  if ev.key != keyNone or ev.scrollDelta != 0 or ev.resized or
      ev.mouse != mouseNone:
    discard handleEvent(tui, ev)
    return true
  tui.scrollbackDirty

proc checkInterrupt*(tui: var TUI) =
  ## Drain pending input without waiting. Long waits belong on the child fd.
  if tui.pollBusy(0):
    tui.render()
