## Minimal terminal control for niminal.
##
## Owns raw mode, alternate screen, cursor, size, and mouse tracking enable.
## Input decoding lives in ui/input.nim.

when defined(windows):
  {.error: "niminal does not support native Windows; use WSL.".}

import std/[os, osproc, posix, streams, strutils, terminal, base64]
import posix/termios
import ../images

type
  TermError* = object of CatchableError

var
  gTermActive = false
  gLastW, gLastH: int

proc measureTerm(): tuple[w, h: int] =
  try:
    result.w = terminalWidth()
  except CatchableError:
    result.w = 80
  try:
    result.h = terminalHeight()
  except CatchableError:
    result.h = 24

proc termWidth*(): int =
  if gLastW <= 0:
    let s = measureTerm()
    gLastW = s.w
    gLastH = s.h
  gLastW

proc termHeight*(): int =
  if gLastH <= 0:
    discard termWidth()
  gLastH

proc termWrite(s: string) =
  stdout.write(s)

proc setCursor*(x, y: int) =
  ## 0-based cursor position.
  setCursorPos(x, y)

proc hideCursor*() =
  termWrite("\e[?25l")

proc showCursor*() =
  termWrite("\e[?25h")

proc clearScreen*() =
  termWrite("\e[2J\e[H")

proc enableMouse*() =
  ## Full SGR mouse like pi fullscreen: wheel scrolls our viewport; drag is
  ## app-owned selection (terminal native select cannot work under mouse
  ## tracking). 1002 reports motion while a button is held.
  termWrite("\e[?1000h\e[?1002h\e[?1006h")

proc disableMouse*() =
  termWrite("\e[?1006l\e[?1002l\e[?1000l\e[?1007l")

proc enableModifyOtherKeys*() =
  ## Ask the terminal to distinguish modified keys (Shift+Enter, etc.).
  ## Level 2 reports ESC [ 27 ; mod ; key ~
  termWrite("\e[>4;2m")
  ## Also request Alt-sends-ESC so Option+Enter becomes ESC CR on macOS.
  termWrite("\e[?1036h")

proc disableModifyOtherKeys*() =
  termWrite("\e[>4;0m")
  termWrite("\e[?1036l")

proc enableBracketedPaste*() =
  termWrite("\e[?2004h")

proc disableBracketedPaste*() =
  termWrite("\e[?2004l")

var
  gOldTermios: Termios
  gRawTermios: Termios

proc useAltScreen(): bool =
  let t = getEnv("TERM")
  t.len == 0 or "xterm" in t or "screen" in t or "tmux" in t or
    "alacritty" in t or "kitty" in t or "rxvt" in t or "vt100" in t

proc enterAltScreen() =
  if useAltScreen():
    termWrite("\e[?1049h")
  else:
    clearScreen()

proc leaveAltScreen() =
  if useAltScreen():
    termWrite("\e[?1049l")
  else:
    clearScreen()

proc termInit*() =
  if gTermActive:
    raise newException(TermError, "terminal already initialised")
  if tcGetAttr(STDIN_FILENO, gOldTermios.addr) != 0:
    raise newException(TermError, "tcgetattr failed")
  gRawTermios = gOldTermios
  # Raw-ish: no echo/canonical; disable ISIG so Ctrl-C arrives as byte 3.
  gRawTermios.c_lflag = gRawTermios.c_lflag and not Cflag(ICANON or ECHO or ISIG)
  gRawTermios.c_cc[VMIN] = 0.char
  gRawTermios.c_cc[VTIME] = 0.char
  if tcSetAttr(STDIN_FILENO, TCSANOW, gRawTermios.addr) != 0:
    raise newException(TermError, "tcsetattr failed")
  gLastW = termWidth()
  gLastH = termHeight()
  enterAltScreen()
  clearScreen()
  hideCursor()
  enableMouse()
  enableModifyOtherKeys()
  enableBracketedPaste()
  gTermActive = true
  stdout.flushFile()

proc termShutdown*() =
  if not gTermActive: return
  disableBracketedPaste()
  disableModifyOtherKeys()
  disableMouse()
  showCursor()
  leaveAltScreen()
  discard tcSetAttr(STDIN_FILENO, TCSANOW, gOldTermios.addr)
  gTermActive = false
  stdout.flushFile()

proc inputPending*(timeoutMs: int): bool =
  var fds: TFdSet
  FD_ZERO(fds)
  FD_SET(STDIN_FILENO, fds)
  if timeoutMs < 0:
    discard select(STDIN_FILENO + 1, fds.addr, nil, nil, nil)  # block forever
  else:
    var tv: Timeval
    tv.tv_sec = Time(timeoutMs div 1000)
    tv.tv_usec = Suseconds(1000 * (timeoutMs mod 1000))
    discard select(STDIN_FILENO + 1, fds.addr, nil, nil, tv.addr)
  FD_ISSET(STDIN_FILENO, fds) != 0

proc readByte*(): int =
  var ch: char
  if read(STDIN_FILENO, ch.addr, 1) > 0:
    return ord(ch)
  -1

proc consumeResize*(): bool =
  ## ioctl size — portable without SIGWINCH (missing on some Nim/mac builds).
  let s = measureTerm()
  if s.w != gLastW or s.h != gLastH:
    gLastW = s.w
    gLastH = s.h
    return true
  false

proc copyToClipboard*(text: string) =
  ## Best-effort clipboard write: OSC 52 (remote-friendly) plus pbcopy on macOS
  ## when the terminal blocks OSC 52 (common in some IDE terminals).
  if text.len == 0: return
  termWrite("\e]52;c;" & encode(text) & "\e\\")
  stdout.flushFile()
  when defined(macosx):
    try:
      let p = startProcess("pbcopy", options = {poUsePath})
      p.inputStream.write(text)
      close(p.inputStream)
      discard waitForExit(p)
      close(p)
    except CatchableError:
      discard

proc captureStdout(cmd: string, args: openArray[string]): string =
  try:
    var p = startProcess(cmd, args = args, options = {poUsePath})
    result = p.outputStream.readAll()
    let code = waitForExit(p)
    close(p)
    if code != 0: result = ""
  except CatchableError:
    result = ""

when defined(macosx):
  const jxaClipboardImage = """
ObjC.import('AppKit');
const pb = $.NSPasteboard.generalPasteboard;
const path = ObjC.unwrap($.NSProcessInfo.processInfo.environment.objectForKey('NIMINAL_CLIP_PATH'));
function save(type) {
  const d = pb.dataForType(type);
  if (d && d.length) { d.writeToFileAtomically(path, true); return true; }
  return false;
}
if (!(save('public.png') || save('public.jpeg') || save('public.gif'))) {
  const d = pb.dataForType('public.tiff');
  if (d && d.length) {
    const img = $.NSImage.alloc.initWithData(d);
    const bitmap = $.NSBitmapImageRep.imageRepWithData(img.TIFFRepresentation);
    const png = bitmap.representationUsingTypeProperties($.NSBitmapImageFileTypePNG, $.NSDictionary.dictionary);
    png.writeToFileAtomically(path, true);
  }
}
"""

  proc osascriptWrote(dest: string, args: openArray[string]): bool =
    try:
      var p = startProcess("osascript", args = args,
        options = {poUsePath, poStdErrToStdOut})
      discard p.outputStream.readAll()
      let code = waitForExit(p)
      close(p)
      return code == 0 and fileExists(dest) and getFileSize(dest) > 0
    except CatchableError:
      return false

  proc jxaClipboardToFile(dest: string): bool =
    putEnv("NIMINAL_CLIP_PATH", dest)
    osascriptWrote(dest, ["-l", "JavaScript", "-e", jxaClipboardImage])

  proc osascriptClipClass(dest, klass: string): bool =
    putEnv("NIMINAL_CLIP_PATH", dest)
    let wrapped = "\u00ABclass " & klass & "\u00BB"
    let script =
      "set p to system attribute \"NIMINAL_CLIP_PATH\"\n" &
      "try\n" &
      "  set d to (the clipboard as " & wrapped & ")\n" &
      "  set f to open for access POSIX file p with write permission\n" &
      "  set eof of f to 0\n" &
      "  write d to f\n" &
      "  close access f\n" &
      "on error\n" &
      "  try\n    close access POSIX file p\n  end try\n" &
      "  error number 1\n" &
      "end try\n"
    osascriptWrote(dest, ["-e", script])

proc readClipboardImage*(): string =
  ## Raw image bytes from the OS clipboard, or empty.
  let tmp = getTempDir() / ("niminal-clip-" & $getCurrentProcessId())
  when defined(macosx):
    try:
      if jxaClipboardToFile(tmp) or osascriptClipClass(tmp, "PNGf") or
          osascriptClipClass(tmp, "JPEG"):
        result = readFile(tmp)
        if sniffImageMime(result).len == 0: result = ""
    finally:
      if fileExists(tmp): removeFile(tmp)
  else:
    for args in [
      @["wl-paste", "--type", "image/png"],
      @["wl-paste", "--type", "image/jpeg"],
      @["xclip", "-selection", "clipboard", "-t", "image/png", "-o"],
      @["xclip", "-selection", "clipboard", "-t", "image/jpeg", "-o"]
    ]:
      let raw = captureStdout(args[0], args[1 .. ^1])
      if sniffImageMime(raw).len > 0:
        return raw

proc readClipboardText*(): string =
  when defined(macosx):
    result = captureStdout("pbpaste", [])
  else:
    result = captureStdout("wl-paste", ["--no-newline"])
    if result.len == 0:
      result = captureStdout("xclip", ["-selection", "clipboard", "-o"])

proc termActive*(): bool =
  gTermActive

proc interruptFd*(): cint =
  ## stdin while the TUI owns it, else -1. Wait alongside a child so
  ## Ctrl-C wakes a blocked tool without polling.
  if gTermActive: STDIN_FILENO else: -1

proc watchingInterrupt*(): bool =
  ## True when a blocked tool should wait on stdin for Ctrl-C.
  gTermActive
