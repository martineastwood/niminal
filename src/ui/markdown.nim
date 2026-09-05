## Minimal markdown-to-ANSI renderer.
##
## Renders a complete response text once. No streaming, no redraws, no timers.
## Supports the subset that shows up regularly in coding-agent replies:
## headings, fenced code blocks, inline code, bold, italic, links, lists,
## blockquotes, horizontal rules, and tables.

import std/[strutils, sequtils]
from std/unicode import runeLen

type
  RenderState = enum
    rsNormal
    rsCodeBlock
    rsTable

proc renderInline(text: string, useColor: bool): string =
  ## Render inline markdown: `code`, ***bold italic***, **bold**, *italic*, links.
  result = text
  if not useColor:
    var i = 0
    var acc = ""
    while i < result.len:
      if result[i] == '`':
        let close = result.find('`', i + 1)
        if close > 0:
          acc.add result[i + 1 ..< close]
          i = close + 1
          continue
      if i + 2 < result.len and result[i] == '*' and result[i + 1] == '*' and result[i + 2] == '*':
        let close = result.find("***", i + 3)
        if close > 0:
          acc.add result[i + 3 ..< close]
          i = close + 3
          continue
      if i + 1 < result.len and result[i] == '*' and result[i + 1] == '*':
        let close = result.find("**", i + 2)
        if close > 0:
          acc.add result[i + 2 ..< close]
          i = close + 2
          continue
      if i + 1 < result.len and result[i] == '*' and result[i + 1] != '*':
        let close = result.find('*', i + 1)
        if close > 0:
          acc.add result[i + 1 ..< close]
          i = close + 1
          continue
      if result[i] == '[':
        let closeBr = result.find(']', i + 1)
        if closeBr > 0 and closeBr + 1 < result.len and result[closeBr + 1] == '(':
          let closeParen = result.find(')', closeBr + 2)
          if closeParen > 0:
            let label = result[i + 1 ..< closeBr]
            let url = result[closeBr + 2 ..< closeParen]
            acc.add label & " (" & url & ")"
            i = closeParen + 1
            continue
      acc.add result[i]
      inc i
    result = acc
    return

  # Colored rendering — check *** before ** before *.
  var i = 0
  var acc = ""
  while i < result.len:
    if result[i] == '`':
      let close = result.find('`', i + 1)
      if close > 0:
        acc.add "\x1b[33m" & result[i + 1 ..< close] & "\x1b[0m"
        i = close + 1
        continue
    if i + 2 < result.len and result[i] == '*' and result[i + 1] == '*' and result[i + 2] == '*':
      let close = result.find("***", i + 3)
      if close > 0:
        # Bold + italic + bright yellow so it stays visible on common terminals.
        acc.add "\x1b[1;3;93m" & result[i + 3 ..< close] & "\x1b[0m"
        i = close + 3
        continue
    if i + 1 < result.len and result[i] == '*' and result[i + 1] == '*':
      let close = result.find("**", i + 2)
      if close > 0:
        acc.add "\x1b[1;93m" & result[i + 2 ..< close] & "\x1b[0m"
        i = close + 2
        continue
    if i + 1 < result.len and result[i] == '*' and result[i + 1] != '*':
      let close = result.find('*', i + 1)
      if close > 0:
        acc.add "\x1b[3m" & result[i + 1 ..< close] & "\x1b[0m"
        i = close + 1
        continue
    if result[i] == '[':
      let closeBr = result.find(']', i + 1)
      if closeBr > 0 and closeBr + 1 < result.len and result[closeBr + 1] == '(':
        let closeParen = result.find(')', closeBr + 2)
        if closeParen > 0:
          let label = result[i + 1 ..< closeBr]
          let url = result[closeBr + 2 ..< closeParen]
          acc.add "\x1b[4m" & label & "\x1b[0m" & " \x1b[2m" & url & "\x1b[0m"
          i = closeParen + 1
          continue
    acc.add result[i]
    inc i
  result = acc

proc parseTableRow(line: string): seq[string] =
  ## Split a markdown table row into cells.
  let trimmed = line.strip
  var s = trimmed
  if s.startsWith("|"): s = s[1 .. ^1]
  if s.endsWith("|"): s = s[0 ..< s.high]
  for cell in s.split("|"):
    result.add cell.strip

proc isTableSeparator(line: string): bool =
  ## Check if a line is a table separator like |---|---|
  let cells = parseTableRow(line)
  if cells.len == 0: return false
  for cell in cells:
    if cell.len == 0: return false
    for ch in cell:
      if ch != '-' and ch != ':' and ch != ' ': return false
  true

proc renderTable(rows: seq[seq[string]], useColor: bool): seq[string] =
  ## Render a markdown table with Unicode box-drawing characters.
  if rows.len == 0: return

  let numCols = rows[0].len
  # Calculate column widths
  var colWidths = newSeq[int](numCols)
  for row in rows:
    for c, cell in row:
      if c < numCols:
        let visible = renderInline(cell, false)
        colWidths[c] = max(colWidths[c], runeLen(visible))

  let topBorder = "┌" & colWidths.mapIt("─".repeat(it + 2)).join("┬") & "┐"
  let midBorder = "├" & colWidths.mapIt("─".repeat(it + 2)).join("┼") & "┤"
  let botBorder = "└" & colWidths.mapIt("─".repeat(it + 2)).join("┴") & "┘"

  if useColor:
    result.add "\x1b[2m" & topBorder & "\x1b[0m"
  else:
    result.add topBorder

  for r, row in rows:
    var line = "│"
    for c in 0 ..< numCols:
      let cell = if c < row.len: row[c] else: ""
      let rendered = renderInline(cell, useColor)
      let pad = colWidths[c] - runeLen(renderInline(cell, false))
      let content = " " & rendered & " ".repeat(pad) & " "
      if r == 0 and useColor:
        line.add "\x1b[1;93m" & content & "\x1b[0m│"
      else:
        line.add content & "│"
    result.add line
    if r == 0:
      if useColor:
        result.add "\x1b[2m" & midBorder & "\x1b[0m"
      else:
        result.add midBorder

  if useColor:
    result.add "\x1b[2m" & botBorder & "\x1b[0m"
  else:
    result.add botBorder

proc renderMarkdown*(text: string, useColor: bool): string =
  ## Render a markdown response to a string with optional ANSI colors.
  var lines = text.splitLines
  var rendered: seq[string] = @[]
  var state = rsNormal
  var codeLines: seq[string] = @[]
  var codeLang = ""
  var tableRows: seq[seq[string]]

  for line in lines:
    case state
    of rsNormal:
      if line.startsWith("```"):
        state = rsCodeBlock
        codeLang = line[3 .. ^1].strip
        codeLines = @[]
        continue
      # Detect table: line starts with | and next line is a separator
      if line.strip.startsWith("|"):
        state = rsTable
        tableRows = @[parseTableRow(line)]
        continue
      if line.strip == "---" or line.strip == "***":
        if useColor:
          rendered.add "\x1b[2m" & "─".repeat(40) & "\x1b[0m"
        else:
          rendered.add "─".repeat(40)
        continue
      if line.startsWith("# "):
        let body = renderInline(line[2 .. ^1].strip, useColor)
        rendered.add (if useColor: "\x1b[1m\x1b[35m" & body & "\x1b[0m" else: body)
      elif line.startsWith("## "):
        let body = renderInline(line[3 .. ^1].strip, useColor)
        rendered.add (if useColor: "\x1b[1;93m" & body & "\x1b[0m" else: body)
      elif line.startsWith("### "):
        let body = renderInline(line[4 .. ^1].strip, useColor)
        rendered.add (if useColor: "\x1b[1;93m" & body & "\x1b[0m" else: body)
      elif line.startsWith("> "):
        let body = renderInline(line[2 .. ^1], useColor)
        rendered.add (if useColor: "\x1b[2m" & "│ " & body & "\x1b[0m" else: "│ " & body)
      elif line.len >= 2 and line[0] in {'-', '*'} and line[1] == ' ':
        let body = renderInline(line[2 .. ^1], useColor)
        rendered.add (if useColor: "\x1b[36m" & "• " & "\x1b[0m" & body else: "• " & body)
      elif line.len >= 3 and line[0].isDigit and line[1] == '.' and line[2] == ' ':
        let dot = line.find(". ")
        let num = line[0 ..< dot]
        let body = renderInline(line[dot + 2 .. ^1], useColor)
        rendered.add (if useColor: "\x1b[36m" & num & ".\x1b[0m " & body else: num & ". " & body)
      else:
        rendered.add renderInline(line, useColor)
    of rsCodeBlock:
      if line.startsWith("```"):
        if useColor:
          rendered.add "\x1b[2m" & codeLines.join("\n") & "\x1b[0m"
        else:
          rendered.add codeLines.join("\n")
        state = rsNormal
        codeLines = @[]
        codeLang = ""
      else:
        codeLines.add line
    of rsTable:
      if line.strip.startsWith("|"):
        tableRows.add parseTableRow(line)
      else:
        # Table ended — render it
        rendered.add renderTable(tableRows, useColor)
        tableRows = @[]
        state = rsNormal
        # Re-process this line in normal mode
        if line.strip.len > 0:
          if line.startsWith("```"):
            state = rsCodeBlock
            codeLang = line[3 .. ^1].strip
            codeLines = @[]
          else:
            rendered.add renderInline(line, useColor)

  # Unterminated code block
  if state == rsCodeBlock and codeLines.len > 0:
    if useColor:
      rendered.add "\x1b[2m" & codeLines.join("\n") & "\x1b[0m"
    else:
      rendered.add codeLines.join("\n")

  # Unterminated table
  if state == rsTable and tableRows.len > 0:
    rendered.add renderTable(tableRows, useColor)

  result = rendered.join("\n")
