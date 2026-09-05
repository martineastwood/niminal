## External tools: Unix-style ephemeral processes discovered via tool.json.
##
## At startup we only read manifests. The executable runs only when invoked.
## runExtension is public so lifecycle hooks can reuse the same protocol later.

import std/[algorithm, json, os, osproc, posix, strformat, strutils, times]
when defined(macosx) or defined(freebsd) or defined(netbsd) or
     defined(openbsd) or defined(dragonfly):
  import posix/kqueue
import config
import nimgent
import tools/tool
import ui/term

const
  DefaultTimeout = 30
  TailKeep = 20_000
  TermGiveUpMs = 200
  BuiltinToolNames* = ["bash", "edit", "glob", "grep", "read", "read_skill", "write"]

type
  ExtensionTool* = object
    name*: string
    description*: string
    command*: seq[string]
    timeoutSeconds*: int
    inputSchema*: JsonNode
    dir*: string

  DiscoverResult* = object
    tools*: seq[ExtensionTool]
    warnings*: seq[string]

proc requestStop(p: Process, hard: bool) =
  let pid = Pid(p.processID)
  let sig = if hard: SIGKILL else: SIGTERM
  if getpgid(pid) == pid:
    discard posix.kill(-pid, sig)
  else:
    discard posix.kill(pid, sig)

proc waitProcess(p: Process, timeoutMs: int, watchInterrupt: bool): bool =
  if timeoutMs < 0:
    return false
  when defined(macosx) or defined(freebsd) or defined(netbsd) or
       defined(openbsd) or defined(dragonfly):
    let kqFD = kqueue()
    if kqFD == -1:
      return p.peekExitCode() != -1
    defer: discard posix.close(kqFD)
    var changes: array[2, KEvent]
    var n = 0
    changes[n] = KEvent(ident: p.processID.uint, filter: cshort(EVFILT_PROC),
      flags: EV_ADD.cushort, fflags: NOTE_EXIT)
    inc n
    if watchInterrupt:
      let fd = interruptFd()
      if fd >= 0:
        changes[n] = KEvent(ident: fd.uint, filter: cshort(EVFILT_READ),
          flags: EV_ADD.cushort)
        inc n
    var tmspec: Timespec
    tmspec.tv_sec = posix.Time(timeoutMs div 1000)
    tmspec.tv_nsec = (timeoutMs mod 1000) * 1_000_000
    var ev: KEvent
    while true:
      let count = kevent(kqFD, addr changes[0], n.cint, addr ev, 1, addr tmspec)
      if count < 0:
        if osLastError().cint == EINTR:
          continue
        return p.peekExitCode() != -1
      return count > 0
  else:
    var fds: array[3, TPollfd]
    var n = 0
    fds[n] = TPollfd(fd: p.outputHandle.cint, events: POLLIN or POLLHUP)
    inc n
    let errFd = p.errorHandle.cint
    if errFd != p.outputHandle.cint:
      fds[n] = TPollfd(fd: errFd, events: POLLIN or POLLHUP)
      inc n
    var stdinFd: cint = -1
    if watchInterrupt:
      stdinFd = interruptFd()
      if stdinFd >= 0:
        fds[n] = TPollfd(fd: stdinFd, events: POLLIN)
        inc n
    while true:
      let ready = poll(addr fds[0], Tnfds(n), timeoutMs.cint)
      if ready < 0:
        if osLastError().cint == EINTR:
          continue
        return p.peekExitCode() != -1
      if ready == 0:
        return false
      var buf: array[256, char]
      for i in 0 ..< n:
        if fds[i].fd == stdinFd:
          continue
        if (fds[i].revents and (POLLIN or POLLHUP)) != 0:
          while posix.read(fds[i].fd, addr buf[0], buf.len) > 0:
            discard
      return true

proc stopChild(p: Process) =
  requestStop(p, hard = false)
  let giveUp = epochTime() + TermGiveUpMs / 1000
  while p.peekExitCode == -1:
    let left = int((giveUp - epochTime()) * 1000)
    if left <= 0:
      break
    discard waitProcess(p, left, watchInterrupt = false)
  if p.peekExitCode == -1:
    requestStop(p, hard = true)
    discard p.waitForExit(1000)

type
  WaitEnd = enum
    weExited, weTimeout, weCancelled

proc waitForChild(p: Process, timeout: int):
                 tuple[endKind: WaitEnd, exitCode: int] =
  let deadline = epochTime() + timeout.float
  let watchInterrupt = watchingInterrupt()
  while true:
    let code = p.peekExitCode()
    if code != -1:
      return (weExited, code)
    if cancelRequested():
      stopChild(p)
      return (weCancelled, -1)
    let remaining = deadline - epochTime()
    if remaining <= 0:
      stopChild(p)
      return (weTimeout, -1)
    discard waitProcess(p, max(1, int(remaining * 1000)), watchInterrupt)

proc truncateOutput(output: string, limit: int): string =
  let outputLimit = max(2, limit)
  if output.len <= outputLimit:
    return output
  let keep = min(TailKeep, outputLimit div 2)
  let head = output[0 ..< (outputLimit - keep)]
  let tail = output[^keep .. ^1]
  head & "\n\n[... truncated " & $(output.len - outputLimit) & " bytes ...]\n\n" & tail

proc resolveCommand(toolDir, cmd0: string): string =
  if cmd0.isAbsolute:
    cmd0
  else:
    toolDir / cmd0

proc parseManifest(path: string): tuple[ok: bool, tool: ExtensionTool, err: string] =
  var doc: JsonNode
  try:
    doc = parseJson(readFile(path))
  except CatchableError as e:
    return (false, ExtensionTool(), "invalid JSON: " & e.msg)
  if doc.isNil or doc.kind != JObject:
    return (false, ExtensionTool(), "manifest must be a JSON object")

  let name = if "name" in doc: doc["name"].getStr.strip else: ""
  let description = if "description" in doc: doc["description"].getStr else: ""
  if name.len == 0:
    return (false, ExtensionTool(), "missing name")
  if description.len == 0:
    return (false, ExtensionTool(), "missing description")
  if "command" notin doc or doc["command"].kind != JArray or doc["command"].len == 0:
    return (false, ExtensionTool(), "command must be a nonempty array")
  var command: seq[string] = @[]
  for item in doc["command"]:
    if item.kind != JString or item.getStr.len == 0:
      return (false, ExtensionTool(), "command entries must be nonempty strings")
    command.add item.getStr
  if "input_schema" notin doc or doc["input_schema"].kind != JObject:
    return (false, ExtensionTool(), "input_schema must be a JSON object")

  var timeout = DefaultTimeout
  if "timeout_seconds" in doc:
    if doc["timeout_seconds"].kind != JInt:
      return (false, ExtensionTool(), "timeout_seconds must be an integer")
    timeout = max(1, doc["timeout_seconds"].getInt)

  result.ok = true
  result.tool = ExtensionTool(
    name: name,
    description: description,
    command: command,
    timeoutSeconds: timeout,
    inputSchema: doc["input_schema"],
    dir: path.parentDir)

proc addTools(result: var DiscoverResult, root: string) =
  if not dirExists(root):
    return
  var dirs: seq[string] = @[]
  for kind, path in walkDir(root):
    if kind == pcDir and fileExists(path / "tool.json"):
      dirs.add path
  dirs.sort()
  for dir in dirs:
    let parsed = parseManifest(dir / "tool.json")
    if not parsed.ok:
      result.warnings.add "skipping " & dir & ": " & parsed.err
      continue
    var replaced = false
    for i in 0 ..< result.tools.len:
      if result.tools[i].name.toLowerAscii == parsed.tool.name.toLowerAscii:
        result.tools[i] = parsed.tool
        replaced = true
        break
    if not replaced:
      result.tools.add parsed.tool

proc discoverExtensions*(workspace: string): DiscoverResult =
  ## Later roots override the same tool name: global → `.agent` → `.niminal`.
  addTools(result, niminalConfigDir() / "tools")
  let root = if dirExists(workspace): expandFilename(workspace) else: workspace
  addTools(result, root / ".agent" / "tools")
  addTools(result, root / ".niminal" / "tools")
  result.tools.sort(proc(a, b: ExtensionTool): int =
    let byName = cmp(a.name.toLowerAscii, b.name.toLowerAscii)
    if byName != 0: byName else: cmp(a.dir, b.dir))

proc isBuiltinName*(name: string): bool =
  let lower = name.toLowerAscii
  for b in BuiltinToolNames:
    if b == lower:
      return true
  false

proc runExtension*(ext: ExtensionTool, input: JsonNode, workspace: string,
                   maxOutputBytes = 100_000): ToolResult =
  ## Spawn the tool: stdin = JSON args, stdout must be JSON, cwd = workspace.
  let spawnCmd = resolveCommand(ext.dir, ext.command[0])
  if not fileExists(spawnCmd):
    return ToolResult(output: "extension executable not found: " & spawnCmd, isError: true)
  let spawnArgs = if ext.command.len > 1: ext.command[1 .. ^1] else: @[]

  let start = epochTime()
  let stamp = $getCurrentProcessId() & "-" & $int(start * 1_000_000)
  let stdinPath = getTempDir() / ("niminal-ext-" & stamp & ".in")
  let stdoutPath = getTempDir() / ("niminal-ext-" & stamp & ".out")
  let stderrPath = getTempDir() / ("niminal-ext-" & stamp & ".err")
  defer:
    if fileExists(stdinPath): removeFile(stdinPath)
    if fileExists(stdoutPath): removeFile(stdoutPath)
    if fileExists(stderrPath): removeFile(stderrPath)

  let payload = if input.isNil: newJObject() else: input
  writeFile(stdinPath, $payload)

  # Redirect via the shell so stdout/stderr cannot fill a pipe and deadlock.
  var argvQuoted = quoteShell(spawnCmd)
  for a in spawnArgs:
    argvQuoted.add " "
    argvQuoted.add quoteShell(a)
  let shell = getEnv("SHELL", "/bin/sh")
  let script = argvQuoted & " <" & quoteShell(stdinPath) &
    " >" & quoteShell(stdoutPath) & " 2>" & quoteShell(stderrPath)
  let shellArgs = @["-c", script]
  var spawnShell = shell
  var spawnShellArgs = shellArgs
  var spawnOpts = {poUsePath}
  when defined(linux):
    spawnShell = "setsid"
    spawnShellArgs = @[shell] & shellArgs
  else:
    spawnOpts.incl poDaemon

  let p = startProcess(
    command = spawnShell,
    args = spawnShellArgs,
    options = spawnOpts,
    workingDir = workspace)
  when not defined(linux):
    let pid = Pid(p.processID)
    discard setpgid(pid, pid)

  let (endKind, exitCode) = waitForChild(p, ext.timeoutSeconds)
  let elapsed = epochTime() - start
  p.close()
  let ms = int(elapsed * 1000)

  var stdout = if fileExists(stdoutPath): readFile(stdoutPath) else: ""
  var stderr = if fileExists(stderrPath): readFile(stderrPath) else: ""
  stdout = truncateOutput(stdout, maxOutputBytes)
  stderr = truncateOutput(stderr, maxOutputBytes)

  if endKind == weTimeout:
    var msg = fmt"TIMEOUT after {ext.timeoutSeconds}s ({ms}ms)"
    if stdout.len == 0 and stderr.len > 0:
      msg.add "\n\nstderr:\n" & stderr
    elif stdout.len > 0:
      msg.add "\n\n" & stdout
    return ToolResult(output: msg, isError: true)
  if endKind == weCancelled:
    var msg = fmt"INTERRUPTED ({ms}ms)"
    if stdout.len == 0 and stderr.len > 0:
      msg.add "\n\nstderr:\n" & stderr
    elif stdout.len > 0:
      msg.add "\n\n" & stdout
    return ToolResult(output: msg, isError: true)

  var jsonOk = false
  if stdout.len > 0:
    try:
      discard parseJson(stdout)
      jsonOk = true
    except CatchableError:
      jsonOk = false

  let failed = exitCode != 0 or not jsonOk
  if failed and stdout.len == 0 and stderr.len > 0:
    var msg = if exitCode != 0: fmt"exit_code: {exitCode}" & "\n\nstderr:\n" & stderr
              else: "stdout was not valid JSON\n\nstderr:\n" & stderr
    return ToolResult(output: msg, isError: true)
  if not jsonOk:
    var msg = "stdout was not valid JSON"
    if stdout.len > 0:
      msg.add "\n\n" & stdout
    return ToolResult(output: msg, isError: true)
  ToolResult(output: stdout, isError: exitCode != 0)

proc makeExtensionTool*(ext: ExtensionTool, workspace: string,
                        maxOutputBytes = 100_000): (ToolDefinition, ToolProc) =
  let def = ToolDefinition(
    name: ext.name,
    description: ext.description,
    inputSchema: ext.inputSchema)
  let captured = ext
  proc run(input: JsonNode): ToolResult =
    runExtension(captured, input, workspace, maxOutputBytes)
  (def, run)

proc registerExtensions*(reg: var ToolRegistry, workspace: string,
                         maxOutputBytes = 100_000): seq[string] =
  ## Discover and register external tools. Built-in names always win.
  ## Returns warnings (invalid manifests, builtin collisions).
  let discovered = discoverExtensions(workspace)
  result = discovered.warnings
  for ext in discovered.tools:
    if isBuiltinName(ext.name):
      result.add "skipping extension '" & ext.name &
        "': name collides with a built-in tool"
      continue
    let pair = makeExtensionTool(ext, workspace, maxOutputBytes)
    reg.register(pair[0], pair[1])
