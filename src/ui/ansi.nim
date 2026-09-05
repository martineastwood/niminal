## ANSI-aware string walking, wrapping, and slicing for the TUI.

from std/unicode import Rune, runeLen, fastRuneAt, runeSubStr

proc skipAnsi*(s: string, i: var int): bool =
  ## If `s[i]` starts an ANSI sequence, advance `i` past it and return true.
  if i + 1 >= s.len or s[i] != '\x1b':
    return false
  if s[i + 1] == '[':
    i += 2
    while i < s.len and s[i] != 'm':
      inc i
    if i < s.len: inc i
    return true
  if s[i + 1] == ']':
    i += 2
    while i < s.len and s[i] != '\x07':
      inc i
    if i < s.len: inc i
    return true
  false

proc stripAnsi*(s: string): string =
  result = newStringOfCap(s.len)
  var i = 0
  while i < s.len:
    let start = i
    if skipAnsi(s, i):
      continue
    result.add s[start]
    inc i

proc ansiVisibleWidth*(s: string): int =
  ## Terminal columns ≈ rune count, ignoring ANSI escapes.
  var i = 0
  while i < s.len:
    if skipAnsi(s, i):
      continue
    var r: Rune
    fastRuneAt(s, i, r)
    inc result

proc truncateAnsi*(s: string, maxWidth: int): string =
  ## Fit to one row; used for the status bar only.
  var visible = 0
  var i = 0
  result = newStringOfCap(s.len)
  while i < s.len and visible < maxWidth:
    let start = i
    if skipAnsi(s, i):
      result.add s[start ..< i]
      continue
    var r: Rune
    fastRuneAt(s, i, r)
    result.add s[start ..< i]
    inc visible
  result.add "\x1b[0m"

proc wrapAnsi*(s: string, maxWidth: int): seq[string] =
  ## Word-wrap a line to `maxWidth` visible columns, preserving ANSI codes.
  if maxWidth <= 0:
    return @[s]
  if ansiVisibleWidth(s) <= maxWidth:
    return @[s]

  result = @[]
  var lineStart = 0
  var i = 0
  var visible = 0
  var lastBreak = -1

  while i < s.len:
    if skipAnsi(s, i):
      continue

    let runeStart = i
    var r: Rune
    fastRuneAt(s, i, r)
    let isSpace = s[runeStart] == ' '

    if visible + 1 > maxWidth:
      var breakAt: int
      var nextStart: int
      if lastBreak >= lineStart:
        breakAt = lastBreak
        nextStart = lastBreak + 1
        while nextStart < s.len:
          if skipAnsi(s, nextStart): continue
          var sr: Rune
          let before = nextStart
          fastRuneAt(s, nextStart, sr)
          if s[before] == ' ':
            discard sr
            continue
          nextStart = before
          break
      else:
        breakAt = runeStart
        nextStart = runeStart
      result.add s[lineStart ..< breakAt] & "\x1b[0m"
      lineStart = nextStart
      i = nextStart
      visible = 0
      lastBreak = -1
      continue

    if isSpace:
      lastBreak = runeStart
    inc visible

  if lineStart < s.len or result.len == 0:
    result.add s[lineStart .. ^1]

proc wrapRunes*(s: string, width: int): seq[string] =
  ## Hard-wrap `s` at `width` columns. Empty input is one empty row.
  if width <= 0:
    return @[s]
  if s.len == 0:
    return @[""]
  var acc = ""
  var vis = 0
  var i = 0
  while i < s.len:
    let start = i
    var r: Rune
    fastRuneAt(s, i, r)
    if vis >= width:
      result.add acc
      acc = ""
      vis = 0
    acc.add s[start ..< i]
    inc vis
  if acc.len > 0:
    result.add acc
  elif result.len == 0:
    result.add ""

proc visibleSlice*(s: string, startCol, endCol: int): string =
  ## Substring covering visible columns [startCol, endCol).
  if endCol <= startCol: return ""
  var visible = 0
  var i = 0
  var startByte = -1
  var endByte = s.len
  while i < s.len:
    if skipAnsi(s, i):
      continue
    let runeStart = i
    var r: Rune
    fastRuneAt(s, i, r)
    if visible == startCol and startByte < 0:
      startByte = runeStart
    if visible >= endCol:
      endByte = runeStart
      break
    inc visible
  if startByte < 0: return ""
  s[startByte ..< endByte]

proc peelBoxGutter*(line: string): tuple[prefix, body: string, gutterCols: int] =
  ## Peel a left rail (`▌ ` user card or `│ `) so wrapped rows keep it.
  result = (prefix: "", body: line, gutterCols: 0)
  var i = 0
  while skipAnsi(line, i):
    discard
  for bar in ["▌", "│"]:
    if i + bar.len > line.len or line[i ..< i + bar.len] != bar:
      continue
    var j = i + bar.len
    while skipAnsi(line, j):
      discard
    var cols = 1
    if j < line.len and line[j] == ' ':
      inc j
      cols = 2
    while skipAnsi(line, j):
      discard
    result.gutterCols = cols
    result.prefix = line[0 ..< j]
    result.body = line[j .. ^1]
    return

proc writeLineWithSelection*(line: string, selStart, selEnd: int) =
  ## Paint one viewport row; [selStart, selEnd) columns in reverse video.
  if selEnd <= selStart:
    stdout.write(line)
    return
  let prefix = visibleSlice(line, 0, selStart)
  let mid = visibleSlice(line, selStart, selEnd)
  let suffix = visibleSlice(line, selEnd, 1_000_000)
  stdout.write(prefix)
  stdout.write("\e[7m")
  stdout.write(stripAnsi(mid))
  stdout.write("\e[27m")
  stdout.write(suffix)
