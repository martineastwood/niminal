## Small TUI palette: named tokens → SGR strings, compiled once per apply.
##
## Built-in dark at 256-color depth matches the historical hardcoded escapes so
## existing tests stay green. Custom themes are JSON under ~/.niminal/themes and
## .niminal/themes. No hot reload, no OSC queries, no per-frame color math.

import std/[json, os, strutils, terminal]
import ../config

type
  ColorDepth* = enum
    cdNone
    cd16
    cd256
    cdTrue

  ## Authored color before depth compile: "", "#rrggbb", or "0".."255".
  ## The dim token may also be the literal "dim" (SGR attribute 2).
  ThemeSpec* = object
    name*: string
    accent*, success*, error*, warning*, muted*, dim*, text*: string
    heading*, model*, panelBg*, selectedBg*, selectedFg*: string

  Theme* = object
    name*: string
    accent*, success*, error*, warning*, muted*, dim*, text*: string
    heading*, model*, panelBg*, selectedBg*, selectedFg*: string
    boldAccent*, boldError*: string
    reset*: string

const
  TokenNames = [
    "accent", "success", "error", "warning", "muted", "dim", "text",
    "heading", "model", "panelBg", "selectedBg", "selectedFg"
  ]

  ## Exact historical dark palette at 256 depth (do not restyle casually).
  Dark256* = Theme(
    name: "dark",
    accent: "\e[36m",
    success: "\e[32m",
    error: "\e[31m",
    warning: "\e[33m",
    muted: "\e[90m",
    dim: "\e[2m",
    text: "\e[37m",
    heading: "\e[1;93m",
    model: "\e[35m",
    panelBg: "\e[48;5;236m",
    selectedBg: "\e[48;5;81m",
    selectedFg: "\e[30m",
    boldAccent: "\e[1;36m",
    boldError: "\e[1;31m",
    reset: "\e[0m"
  )

  DarkSpec* = ThemeSpec(
    name: "dark",
    accent: "#00afaf",
    success: "#00af00",
    error: "#af0000",
    warning: "#afaf00",
    muted: "242",
    dim: "dim",
    text: "#c6c6c6",
    heading: "#ffff5f",
    model: "#af00af",
    panelBg: "#303030",
    selectedBg: "#5fd7ff",
    selectedFg: "#000000"
  )

  LightSpec* = ThemeSpec(
    name: "light",
    accent: "#005f87",
    success: "#008700",
    error: "#af0000",
    warning: "#af5f00",
    muted: "245",
    dim: "dim",
    text: "#262626",
    heading: "#5f00af",
    model: "#8700af",
    panelBg: "#e4e4e4",
    selectedBg: "#0087af",
    selectedFg: "#ffffff"
  )

var currentTheme* = Dark256
var activeDepth = cd256

proc colorsOn*(t: Theme): bool =
  t.reset.len > 0

proc paint*(t: Theme, code, text: string): string =
  if code.len == 0 or t.reset.len == 0: return text
  code & text & t.reset

proc italicHeading*(t: Theme): string =
  ## Prefer a single SGR (`\e[1;93m` → `\e[1;3;93m`); else prefix italic.
  let h = t.heading
  if h.len >= 5 and h.startsWith("\e[1;") and h[^1] == 'm':
    return "\e[1;3;" & h[4 ..< h.high] & "m"
  if h.len > 0: return "\e[3m" & h
  if t.colorsOn: return "\e[3m"
  ""

proc detectDepth*(): ColorDepth =
  if getEnv("NO_COLOR").len > 0: return cdNone
  if not stdout.isatty: return cdNone
  let ct = getEnv("COLORTERM").toLowerAscii
  if ct == "truecolor" or ct == "24bit": return cdTrue
  let term = getEnv("TERM").toLowerAscii
  if term.len == 0 or term == "dumb": return cd16
  if "256color" in term or "truecolor" in term: return cd256
  if term.startsWith("xterm") or term.startsWith("screen") or
     term.startsWith("tmux") or "alacritty" in term or "kitty" in term or
     "rxvt" in term or "vt220" in term or "color" in term:
    return cd256
  cd16

proc prefersLightBackground(): bool =
  ## COLORFGBG is `fg;bg` with 0-15 ANSI indices. Bright bg (≥8) ⇒ light.
  let cfg = getEnv("COLORFGBG")
  if cfg.len == 0: return false
  let parts = cfg.split(';')
  if parts.len < 2: return false
  try:
    result = parseInt(parts[^1].strip) >= 8
  except ValueError:
    result = false

proc resolveThemeName(name: string): string =
  let n = name.strip.toLowerAscii
  if n.len == 0 or n == "auto":
    if prefersLightBackground(): return "light"
    return "dark"
  n

proc parseHexRgb(s: string): tuple[ok: bool, r, g, b: int] =
  if s.len != 7 or s[0] != '#': return
  try:
    result.r = parseHexInt(s[1 .. 2])
    result.g = parseHexInt(s[3 .. 4])
    result.b = parseHexInt(s[5 .. 6])
    result.ok = result.r in 0 .. 255 and result.g in 0 .. 255 and
                result.b in 0 .. 255
  except ValueError:
    discard

proc rgbToAnsi16(r, g, b: int): int =
  ## Nearest of the 16 basic ANSI colors (0-15).
  let bright = max(r, max(g, b)) >= 180
  var idx = 0
  if r >= 128: idx = idx or 1
  if g >= 128: idx = idx or 2
  if b >= 128: idx = idx or 4
  if bright and idx > 0: idx = idx or 8
  elif r + g + b < 48: idx = 0
  elif r + g + b > 600: idx = 15
  idx

proc rgbTo256(r, g, b: int): int =
  ## Nearest xterm 256-color index (cube or grayscale).
  if r == g and g == b:
    if r < 8: return 16
    if r > 248: return 231
    return 232 + ((r - 8) * 24) div 247
  let ri = (r * 5) div 255
  let gi = (g * 5) div 255
  let bi = (b * 5) div 255
  16 + 36 * ri + 6 * gi + bi

proc sgrFg16(idx: int): string =
  if idx < 8: "\e[" & $(30 + idx) & "m"
  else: "\e[" & $(90 + idx - 8) & "m"

proc sgrBg16(idx: int): string =
  if idx < 8: "\e[" & $(40 + idx) & "m"
  else: "\e[" & $(100 + idx - 8) & "m"

proc sgrFg256(idx: int): string = "\e[38;5;" & $idx & "m"
proc sgrBg256(idx: int): string = "\e[48;5;" & $idx & "m"
proc sgrFgTrue(r, g, b: int): string = "\e[38;2;" & $r & ";" & $g & ";" & $b & "m"
proc sgrBgTrue(r, g, b: int): string = "\e[48;2;" & $r & ";" & $g & ";" & $b & "m"

proc compileColor(raw: string, depth: ColorDepth, background: bool): string =
  if depth == cdNone: return ""
  let v = raw.strip
  if v.len == 0: return ""
  if v == "dim":
    return if background: "" else: "\e[2m"
  let hex = parseHexRgb(v)
  if hex.ok:
    case depth
    of cdNone: return ""
    of cdTrue:
      return if background: sgrBgTrue(hex.r, hex.g, hex.b)
             else: sgrFgTrue(hex.r, hex.g, hex.b)
    of cd256:
      let idx = rgbTo256(hex.r, hex.g, hex.b)
      return if background: sgrBg256(idx) else: sgrFg256(idx)
    of cd16:
      let idx = rgbToAnsi16(hex.r, hex.g, hex.b)
      return if background: sgrBg16(idx) else: sgrFg16(idx)
  try:
    let idx = parseInt(v)
    if idx < 0 or idx > 255: return ""
    case depth
    of cdNone: return ""
    of cd16:
      var a = idx
      if idx >= 16:
        var r, g, b: int
        if idx >= 232:
          let gray = 8 + (idx - 232) * 10
          r = gray; g = gray; b = gray
        else:
          let c = idx - 16
          r = (c div 36) * 51
          g = ((c div 6) mod 6) * 51
          b = (c mod 6) * 51
        a = rgbToAnsi16(r, g, b)
      return if background: sgrBg16(a) else: sgrFg16(a)
    of cd256, cdTrue:
      return if background: sgrBg256(idx) else: sgrFg256(idx)
  except ValueError:
    return ""

proc compileBold(fg: string): string =
  ## Prefer a single SGR when fg is a simple `\e[…m`; else prefix bold.
  if fg.len == 0: return ""
  if fg.len >= 3 and fg[0] == '\e' and fg[1] == '[' and fg[^1] == 'm' and
     "38;2;" notin fg and "48;2;" notin fg and "38;5;" notin fg and
     "48;5;" notin fg:
    # \e[36m → \e[1;36m ; \e[1;93m stays as-is if already bold
    let body = fg[2 ..< fg.high]
    if body.startsWith("1;"): return fg
    return "\e[1;" & body & "m"
  "\e[1m" & fg

proc compileTheme*(spec: ThemeSpec, depth: ColorDepth): Theme =
  if depth == cdNone:
    return Theme(name: spec.name)
  result.name = spec.name
  result.accent = compileColor(spec.accent, depth, false)
  result.success = compileColor(spec.success, depth, false)
  result.error = compileColor(spec.error, depth, false)
  result.warning = compileColor(spec.warning, depth, false)
  result.muted = compileColor(spec.muted, depth, false)
  result.dim = compileColor(spec.dim, depth, false)
  result.text = compileColor(spec.text, depth, false)
  result.heading = compileColor(spec.heading, depth, false)
  if result.heading.len > 0 and not result.heading.contains("1;"):
    result.heading = compileBold(result.heading)
  result.model = compileColor(spec.model, depth, false)
  if depth in {cd256, cdTrue}:
    result.panelBg = compileColor(spec.panelBg, depth, true)
    result.selectedBg = compileColor(spec.selectedBg, depth, true)
    result.selectedFg = compileColor(spec.selectedFg, depth, false)
  result.boldAccent = compileBold(result.accent)
  result.boldError = compileBold(result.error)
  result.reset = "\e[0m"

proc tokenSet(spec: var ThemeSpec, key, value: string) =
  case key
  of "accent": spec.accent = value
  of "success": spec.success = value
  of "error": spec.error = value
  of "warning": spec.warning = value
  of "muted": spec.muted = value
  of "dim": spec.dim = value
  of "text": spec.text = value
  of "heading": spec.heading = value
  of "model": spec.model = value
  of "panelBg": spec.panelBg = value
  of "selectedBg": spec.selectedBg = value
  of "selectedFg": spec.selectedFg = value
  else: discard

proc colorNodeToStr(n: JsonNode): string =
  case n.kind
  of JString: n.getStr
  of JInt: $n.getInt
  of JFloat: $n.getInt
  else: ""

proc parseThemeJson*(doc: JsonNode): tuple[ok: bool, spec: ThemeSpec, err: string] =
  if doc.isNil or doc.kind != JObject:
    return (false, ThemeSpec(), "theme must be a JSON object")
  let name = if "name" in doc and doc["name"].kind == JString: doc["name"].getStr.strip
             else: ""
  if name.len == 0:
    return (false, ThemeSpec(), "theme name is required")
  if '/' in name:
    return (false, ThemeSpec(), "theme name must not contain '/'")
  if "colors" notin doc or doc["colors"].kind != JObject:
    return (false, ThemeSpec(), "theme colors object is required")
  let colors = doc["colors"]
  var spec = ThemeSpec(name: name)
  var seen: seq[string]
  for k, v in colors:
    if k notin TokenNames:
      return (false, ThemeSpec(), "unknown color token '" & k & "'")
    spec.tokenSet(k, colorNodeToStr(v))
    seen.add k
  for t in TokenNames:
    if t notin seen:
      return (false, ThemeSpec(), "missing color token '" & t & "'")
  (true, spec, "")

proc loadThemeFile(path: string): tuple[ok: bool, spec: ThemeSpec, err: string] =
  if not fileExists(path):
    return (false, ThemeSpec(), "theme file not found: " & path)
  try:
    result = parseThemeJson(parseJson(readFile(path)))
  except CatchableError as e:
    result = (false, ThemeSpec(), e.msg)

proc themeSearchRoots(workspace: string): seq[string] =
  result.add niminalConfigDir() / "themes"
  if workspace.len > 0:
    let root = if dirExists(workspace): expandFilename(workspace) else: workspace
    result.add root / ".niminal" / "themes"

proc discoverThemes(workspace: string): seq[ThemeSpec] =
  ## Later roots win by case-insensitive name.
  for root in themeSearchRoots(workspace):
    if not dirExists(root): continue
    for kind, path in walkDir(root):
      if kind != pcFile or not path.endsWith(".json"): continue
      let loaded = loadThemeFile(path)
      if not loaded.ok: continue
      let key = loaded.spec.name.toLowerAscii
      var replaced = false
      for i in 0 ..< result.len:
        if result[i].name.toLowerAscii == key:
          result[i] = loaded.spec
          replaced = true
          break
      if not replaced:
        result.add loaded.spec

proc listThemeNames*(workspace = ""): seq[string] =
  result = @["auto", "dark", "light"]
  for spec in discoverThemes(workspace):
    if spec.name.toLowerAscii notin ["auto", "dark", "light"]:
      result.add spec.name

proc findUserTheme*(name, workspace: string): tuple[ok: bool, spec: ThemeSpec, err: string] =
  let want = name.toLowerAscii
  for spec in discoverThemes(workspace):
    if spec.name.toLowerAscii == want:
      return (true, spec, "")
  (false, ThemeSpec(), "unknown theme '" & name & "'")

proc builtinSpec(name: string): tuple[found: bool, spec: ThemeSpec] =
  case name.toLowerAscii
  of "dark": (true, DarkSpec)
  of "light": (true, LightSpec)
  else: (false, ThemeSpec())

proc compileNamedTheme*(name: string, depth: ColorDepth,
                        workspace = ""): tuple[ok: bool, theme: Theme, err: string] =
  let resolved = resolveThemeName(name)
  if depth == cdNone:
    return (true, Theme(name: resolved), "")
  # Historical dark/256 bit-identical to pre-theme escapes.
  if resolved == "dark" and depth == cd256:
    return (true, Dark256, "")
  let built = builtinSpec(resolved)
  if built.found:
    return (true, compileTheme(built.spec, depth), "")
  let user = findUserTheme(resolved, workspace)
  if not user.ok:
    return (false, Theme(), user.err)
  (true, compileTheme(user.spec, depth), "")

proc applyTheme*(name: string, depth = activeDepth, workspace = ""): string =
  ## Compile and install `currentTheme`. Returns "" or an error message.
  ## Depth defaults to `activeDepth` so /theme|/new|/resume keep the startup
  ## capability (tests stay on cd256; non-TTY detectDepth would wipe colors).
  let compiled = compileNamedTheme(name, depth, workspace)
  if not compiled.ok:
    return compiled.err
  activeDepth = depth
  currentTheme = compiled.theme
  ""

proc cardRail(t: Theme, fg: string): string =
  if not t.colorsOn:
    return "▌ "
  result = fg & "▌" & t.reset
  result.add if t.panelBg.len > 0: t.panelBg & " " else: " "

proc userRail*(t: Theme = currentTheme): string =
  ## Accent rail + optional panel background (OpenCode-style).
  cardRail(t, t.accent)

proc toolRail*(t: Theme, isError: bool): string =
  cardRail(t, if isError: t.error else: t.accent)
