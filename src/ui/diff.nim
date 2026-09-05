## Display-only edit/write hunks. Not sent to the model.
##
## `-`/`+` prefixes, no unified-diff headers. `edit` is old then new;
## `write` is all additions from `content`.

import std/[json, strutils]

proc prefixLines(text, marker, color: string, useColor: bool): seq[string] =
  if text.len == 0: return
  for line in text.splitLines:
    let body = marker & line
    if useColor:
      # ponytail: no SGR reset — TUI card background stays open until paint.
      result.add color & body
    else:
      result.add body

proc formatToolHunk*(name: string, input: JsonNode, useColor: bool): seq[string] =
  ## Empty if this is not a successful-edit/write display, or args are missing.
  if input.isNil or input.kind != JObject:
    return
  case name
  of "edit":
    let reps = input.getOrDefault("replacements")
    if not reps.isNil and reps.kind == JArray and reps.len > 0:
      for r in reps:
        result.add prefixLines(r.getOrDefault("old_text").getStr, "- ", "\e[31m",
          useColor)
        result.add prefixLines(r.getOrDefault("new_text").getStr, "+ ", "\e[32m",
          useColor)
    else:
      result.add prefixLines(input.getOrDefault("old_text").getStr, "- ", "\e[31m",
        useColor)
      result.add prefixLines(input.getOrDefault("new_text").getStr, "+ ", "\e[32m",
        useColor)
  of "write":
    result.add prefixLines(input.getOrDefault("content").getStr, "+ ", "\e[32m",
      useColor)
  else:
    discard
