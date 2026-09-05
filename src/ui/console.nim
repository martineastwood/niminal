## Plain terminal UI. It deliberately performs no redraws, timers or polling.
##
## Colors are ANSI and are disabled automatically when stdout is not a TTY or
## when the user sets NO_COLOR (https://no-color.org/).

import std/[json, os, strutils, terminal]
import nimgent
import ../commands
import markdown
import diff

type
  Color* = enum
    cDefault
    cDim
    cCyan
    cGreen
    cRed
    cYellow
    cMagenta
    cBold

const
  Codes = [
    cDefault: "\x1b[0m",
    cDim: "\x1b[2m",
    cCyan: "\x1b[36m",
    cGreen: "\x1b[32m",
    cRed: "\x1b[31m",
    cYellow: "\x1b[33m",
    cMagenta: "\x1b[35m",
    cBold: "\x1b[1m"
  ]

proc colorsEnabled(): bool =
  if getEnv("NO_COLOR").len > 0: return false
  stdout.isatty

proc color*(s: string, c: Color): string =
  if colorsEnabled(): Codes[c] & s & Codes[cDefault] else: s

proc printPrompt*() =
  stdout.write("> ".color(cCyan))
  stdout.flushFile()

proc printToolStart*(call: ContentBlock) =
  let summary =
    if call.input.isNil: "{}"
    elif call.name == "edit" or call.name == "write":
      call.input.getOrDefault("path").getStr
    else:
      call.input.pretty(0)
  stdout.write ("● " & call.name).color(cCyan) & " " &
    summary.color(cDim) & "\n"
  stdout.flushFile()

proc printToolResult*(output: string, isError: bool,
                      call: ContentBlock = ContentBlock()) =
  if not isError and call.kind == ckToolUse:
    let hunk = formatToolHunk(call.name, call.input, colorsEnabled())
    if hunk.len > 0:
      stdout.write hunk.join("\n")
      if colorsEnabled(): stdout.write("\e[0m")
      stdout.write "\n\n"
      stdout.flushFile()
      return
  if isError:
    stdout.write ("  ERROR: " & output).color(cRed) & "\n\n"
  else:
    stdout.write output.color(cDim) & (if output.endsWith("\n"): "\n" else: "\n\n")
  stdout.flushFile()

proc printResponse*(response: ProviderResponse) =
  let content = response.textContent()
  if content.len > 0:
    let rendered = renderMarkdown(content, colorsEnabled())
    stdout.write rendered & (if rendered.endsWith("\n"): "" else: "\n")
  var stats: seq[string] = @[]
  if response.model.len > 0:
    stats.add response.model.color(cMagenta)
  for label in formatUsageLabels(response.usage):
    stats.add label.color(cDim)
  if stats.len > 0:
    stdout.write stats.join("  ").color(cDim) & "\n"
  stdout.flushFile()

proc printHelp*() =
  stdout.write helpText()
