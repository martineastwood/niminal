## edit tool — exact search-and-replace on a single file.

import std/[json, strutils, strformat, os]
import tool, ../workspace, nimgent

proc makeEditTool*(ws: Workspace): (ToolDefinition, ToolProc) =
  let def = ToolDefinition(
    name: "edit",
    description: "Replace one unique occurrence of old_text with new_text in a file. Supply expected_version (from read) to guard against concurrent changes.",
    inputSchema: %*{
      "type": "object",
      "properties": {
        "path": {"type": "string", "description": "File path relative to workspace root."},
        "old_text": {"type": "string", "description": "Exact text to find. Must occur exactly once."},
        "new_text": {"type": "string", "description": "Replacement text."},
        "expected_version": {"type": "string", "description": "Version hash from a previous read. Edit is rejected if the file has changed since."}
      },
      "required": ["path", "old_text", "new_text"]
    }
  )

  proc run(input: JsonNode): ToolResult =
    let path = input["path"].getStr
    var resolved: string
    try:
      resolved = ws.resolve(path)
    except WorkspaceError as e:
      return ToolResult(output: e.msg, isError: true)

    if not fileExists(resolved):
      return ToolResult(output: "File not found: " & path, isError: true)

    let content = readFile(resolved)
    let currentVersion = hashContent(content)

    if "expected_version" in input:
      let expected = input["expected_version"].getStr
      if expected.len > 0 and expected != currentVersion:
        return ToolResult(
          output: "EDIT_FAILED\n\nFile version changed since previous read.\n" &
                  "expected: " & expected & "\n" &
                  "current:  " & currentVersion & "\n\n" &
                  "Please reread the file before editing.",
          isError: true)

    let oldText = input["old_text"].getStr
    let newText = input["new_text"].getStr
    let count = content.count(oldText)

    if count == 0:
      var nearby = ""
      let lines = content.splitLines
      let needle = oldText.splitLines[0]
      for i, line in lines:
        if needle.len > 8 and needle[0..min(7, needle.high)] in line:
          nearby.add fmt"  {i+1} | {line}" & "\n"
          if nearby.len > 500: break

      var msg = "EDIT_FAILED\n\nold_text was not found exactly.\n"
      msg.add "current version: " & currentVersion & "\n"
      if nearby.len > 0:
        msg.add "\nPossibly similar lines:\n" & nearby
      return ToolResult(output: msg, isError: true)

    if count > 1:
      return ToolResult(
        output: "EDIT_FAILED\n\nold_text matches " & $count & " locations. It must be unique.\n" &
                "Add surrounding context to disambiguate.",
        isError: true)

    let newContent = content.replace(oldText, newText)
    writeFileAtomic(resolved, newContent)
    invalidateWorkspaceFileList()
    let newVersion = hashContent(newContent)

    ToolResult(
      output: fmt"OK — {ws.relative(resolved)}" & "\n" &
              fmt"version: {newVersion}",
      isError: false)

  (def, run)
