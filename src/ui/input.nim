## Shared terminal input decoder for niminal.
##
## Reads bytes from ui/term (posix raw stdin) and turns them into keys /
## mouse events.

import std/strutils
import keys
import term

type
  MouseKind* = enum
    mouseNone
    mousePress    ## left button down
    mouseRelease  ## left button up
    mouseDrag     ## motion while left button held

  InputEvent* = object
    key*: Key
    ch*: char           ## ASCII keyChar; prefer `text` for insert
    text*: string       ## UTF-8 rune or a whole paste
    scrollDelta*: int   ## +N scroll up (older), -N scroll down (newer)
    resized*: bool
    mouse*: MouseKind
    mouseX*: int        ## 0-based column
    mouseY*: int        ## 0-based row

proc readByteWait(ms: int): int =
  if inputPending(ms):
    return readByte()
  -1

proc normalizePasteText*(s: string): string =
  ## CR LF / CR → LF so a paste never submits.
  result = newStringOfCap(s.len)
  var i = 0
  while i < s.len:
    if s[i] == '\r':
      result.add '\n'
      if i + 1 < s.len and s[i + 1] == '\n':
        inc i
    else:
      result.add s[i]
    inc i

proc utf8SeqLen(b: int): int =
  if b <= 0x7F: 1
  elif (b and 0xE0) == 0xC0: 2
  elif (b and 0xF0) == 0xE0: 3
  elif (b and 0xF8) == 0xF0: 4
  else: 1

proc readUtf8Text(first: int): string =
  let n = utf8SeqLen(first)
  result = $char(first)
  for _ in 2 .. n:
    let b = readByteWait(50)
    if b < 0: return
    result.add char(b)

proc charEvent(s: string): InputEvent =
  result.key = keyChar
  result.text = s
  if s.len == 1:
    result.ch = s[0]

proc parseSgrMouse(params: string, press: bool): InputEvent =
  ## SGR mouse: ESC [ < btn ; x ; y M/m  (coords are 1-based).
  let parts = params.split(';')
  if parts.len < 3:
    return
  var btn, x, y: int
  try:
    btn = parseInt(parts[0])
    x = parseInt(parts[1])
    y = parseInt(parts[2])
  except ValueError:
    return
  result.mouseX = max(0, x - 1)
  result.mouseY = max(0, y - 1)

  # Wheel: bit 6 set. Low bit selects direction (0=up, 1=down).
  if (btn and 64) != 0:
    if (btn and 1) != 0:
      result.scrollDelta = -3
    else:
      result.scrollDelta = 3
    return

  let button = btn and 3
  if button != 0:
    return  # ignore middle/right for now

  if not press:
    result.mouse = mouseRelease
  elif (btn and 32) != 0:
    result.mouse = mouseDrag
  else:
    result.mouse = mousePress

proc ctrlKey(b: int): Key =
  case b
  of 1: keyCtrlA
  of 2: keyCtrlB
  of 3: keyCtrlC
  of 5: keyCtrlE
  of 6: keyCtrlF
  of 9: keyTab
  of 14: keyCtrlN
  of 15: keyCtrlO
  of 16: keyCtrlP
  of 22: keyCtrlV
  else: keyNone

proc isModifiedPaste*(seq: string): bool =
  ## Ctrl/Cmd+V as xterm modifyOtherKeys (`27;5;118~`) or CSI-u (`118;5u`).
  var code, mods = 0
  if seq.startsWith("27;") and seq.endsWith("~"):
    let parts = seq[3 ..< seq.len - 1].split(';')
    if parts.len != 2: return false
    try:
      mods = parseInt(parts[0])
      code = parseInt(parts[1])
    except ValueError:
      return false
  elif seq.endsWith("u") and ';' in seq:
    let parts = seq[0 ..< seq.len - 1].split(';')
    if parts.len != 2: return false
    try:
      code = parseInt(parts[0])
      mods = parseInt(parts[1])
    except ValueError:
      return false
  else:
    return false
  if code notin {86, 118}:  # v / V
    return false
  let bits = mods - 1
  bits > 0 and (bits and (4 or 8)) != 0  # ctrl and/or cmd/meta

proc readBracketedPaste(): InputEvent =
  ## Bytes between ESC [ 200 ~ and ESC [ 201 ~.
  var acc = ""
  while true:
    let b = readByteWait(500)
    if b < 0:
      break
    if b == 0x1b:
      let n1 = readByteWait(50)
      if n1 != ord('['):
        continue
      var seq = ""
      while true:
        let x = readByteWait(50)
        if x < 0: break
        seq.add char(x)
        if char(x) == '~': break
      if seq == "201~":
        break
      continue
    if b == 9 or b >= 32 or b == 10 or b == 13:
      acc.add char(b)
  let text = normalizePasteText(acc)
  # Cmd+V of an image often arrives as an empty bracketed paste.
  if text.strip.len == 0:
    result.key = keyCtrlV
    return
  result = charEvent(text)

proc readEscapeSequence(): InputEvent =
  ## Called after ESC has already been consumed.
  let ch2 = readByteWait(50)
  if ch2 < 0:
    result.key = keyEscape
    return
  # Option/Alt+Enter (common on macOS): ESC then CR/LF → newline in composer.
  if ch2 == ord('\r') or ch2 == ord('\n'):
    result.key = keyShiftEnter
    return
  if ch2 == ord('O'):
    let ch3 = readByteWait(50)
    if ch3 < 0: return
    case ch3.char
    of 'A': result.key = keyUp
    of 'B': result.key = keyDown
    of 'C': result.key = keyRight
    of 'D': result.key = keyLeft
    of 'H': result.key = keyHome
    of 'F': result.key = keyEnd
    else: discard
    return
  if ch2 != ord('['):
    return
  let ch3 = readByteWait(50)
  if ch3 < 0: return
  # SGR mouse: ESC [ < btn ; x ; y M/m
  if ch3 == ord('<'):
    var params = ""
    while true:
      let b = readByteWait(50)
      if b < 0: return
      if b == ord('M') or b == ord('m'):
        return parseSgrMouse(params, press = b == ord('M'))
      params.add char(b)
    return
  case ch3.char
  of 'A': result.key = keyUp
  of 'B': result.key = keyDown
  of 'C': result.key = keyRight
  of 'D': result.key = keyLeft
  of 'H': result.key = keyHome
  of 'F': result.key = keyEnd
  of 'Z': discard  # shift-tab
  else:
    var seq = $ch3.char
    while true:
      let b = readByteWait(50)
      if b < 0: return
      let c = char(b)
      seq.add c
      if c in {'~', 'A'..'Z', 'a'..'z'}:
        break
    # Shift+Enter / modified Enter variants (xterm, kitty, modifyOtherKeys).
    if seq in ["13;2~", "13;2u", "27;2;13~", "13;2;13~"]:
      result.key = keyShiftEnter
    elif seq.startsWith("27;") and seq.endsWith(";13~"):
      # ESC [ 27 ; <mods> ; 13 ~  — any modifier+Enter → newline (not bare Enter)
      result.key = keyShiftEnter
    elif isModifiedPaste(seq):
      result.key = keyCtrlV
    elif seq == "200~":
      return readBracketedPaste()
    elif seq == "3~": result.key = keyDelete
    elif seq == "1~" or seq == "7~": result.key = keyHome
    elif seq == "4~" or seq == "8~": result.key = keyEnd
    elif seq == "5~": result.key = keyPageUp
    elif seq == "6~": result.key = keyPageDown

proc readInputEvent*(timeoutMs: int): InputEvent =
  if consumeResize():
    result.resized = true
    return

  if not inputPending(timeoutMs):
    if consumeResize():
      result.resized = true
    else:
      result.key = keyNone
    return

  if consumeResize():
    result.resized = true
    if not inputPending(0):
      return

  let b = readByte()
  if b < 0:
    result.key = keyNone
    return
  if b == 0x1b:
    return readEscapeSequence()
  # Enter is CR and/or LF depending on the terminal — both submit.
  # Newlines come only from Shift/Option+Enter or bracketed paste.
  if b == ord('\r') or b == ord('\n'):
    result.key = keyEnter
    return
  if b == 127 or b == 8:
    result.key = keyBackspace
    return
  if b == 9:
    result.key = keyTab
    return
  if b >= 1 and b <= 26:
    result.key = ctrlKey(b)
    return
  if b >= 32 and b <= 126:
    return charEvent($char(b))
  if b >= 0x80:
    return charEvent(readUtf8Text(b))
  result.key = keyNone
