## bash tool — run a shell command in the workspace.

import std/[json, os, osproc, posix, strformat, times]
when defined(macosx) or defined(freebsd) or defined(netbsd) or
     defined(openbsd) or defined(dragonfly):
  import posix/kqueue
import tool, nimgent, ../ui/term

const
  DefaultTimeout = 120
  MaxOutputBytes = 100_000
  TailKeep = 20_000
  TermGiveUpMs = 200

proc requestStop(p: Process, hard: bool) =
  let pid = Pid(p.processID)
  let sig = if hard: SIGKILL else: SIGTERM
  # Decide at kill time: Linux setsid() can complete after startProcess
  # returns, and kill(-pid) of our own group would take niminal with it.
  if getpgid(pid) == pid:
    discard posix.kill(-pid, sig)
  else:
    discard posix.kill(pid, sig)

proc waitProcess(p: Process, timeoutMs: int, watchInterrupt: bool): bool =
  ## Block until the child exits, interrupt input is ready, or timeoutMs.
  ## True if something happened before the timeout. Nim's timed waitForExit
  ## SIGKILLs on expiry, so we wait ourselves and decide SIGTERM/cancel.
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
      # Drain child pipes so a leftover POLLIN cannot busy-loop. Never drain stdin.
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

proc makeBashTool*(workDir: string,
                   maxOutputBytes = MaxOutputBytes): (ToolDefinition, ToolProc) =
  let outputLimit = max(2, maxOutputBytes)
  let def = ToolDefinition(
    name: "bash",
    description: "Run a shell command. Returns stdout, stderr, exit code, and duration.",
    inputSchema: %*{
      "type": "object",
      "properties": {
        "command": {"type": "string", "description": "Shell command to execute."},
        "timeout_seconds": {"type": "integer", "description": "Timeout in seconds (default 120)."}
      },
      "required": ["command"]
    }
  )

  proc run(input: JsonNode): ToolResult =
    let command = input["command"].getStr
    let timeout = if "timeout_seconds" in input:
      max(1, input["timeout_seconds"].getInt)
    else:
      DefaultTimeout

    let start = epochTime()
    let stamp = $getCurrentProcessId() & "-" & $int(start * 1_000_000)
    let stdoutPath = getTempDir() / ("niminal-" & stamp & ".out")
    let stderrPath = getTempDir() / ("niminal-" & stamp & ".err")
    let redirections = " >" & quoteShell(stdoutPath) &
      " 2>" & quoteShell(stderrPath)
    let shell = getEnv("SHELL", "/bin/sh")
    # set +m: keep `&` children in this process group (zsh job control
    # otherwise orphans them from killpg).
    let shellArgs = @["-c", "set +m; (" & command & ")" & redirections]
    var spawnCmd = shell
    var spawnArgs = shellArgs
    var spawnOpts = {poUsePath}
    when defined(linux):
      # Nim's fork path ignores poDaemon (SETPGROUP), and parent setpgid
      # after exec fails with EACCES. setsid makes the tracked pid a
      # session leader before the shell runs so kill(-pid) reaches
      # grandchildren.
      # ponytail: needs `setsid` on PATH (util-linux). Upgrade: posix_spawn SETPGROUP.
      spawnCmd = "setsid"
      spawnArgs = @[shell] & shellArgs
    else:
      spawnOpts.incl poDaemon
    let p = startProcess(
      command = spawnCmd,
      args = spawnArgs,
      options = spawnOpts,
      workingDir = workDir
    )
    when not defined(linux):
      let pid = Pid(p.processID)
      discard setpgid(pid, pid)

    let (endKind, exitCode) = waitForChild(p, timeout)
    let elapsed = epochTime() - start
    p.close()

    var stdout = if fileExists(stdoutPath): readFile(stdoutPath) else: ""
    var stderr = if fileExists(stderrPath): readFile(stderrPath) else: ""
    if fileExists(stdoutPath): removeFile(stdoutPath)
    if fileExists(stderrPath): removeFile(stderrPath)

    var output = ""
    if stdout.len > 0:
      output.add "stdout:\n" & stdout
    if stderr.len > 0:
      if output.len > 0: output.add "\n"
      output.add "stderr:\n" & stderr

    let ms = int(elapsed * 1000)

    if endKind == weTimeout:
      return ToolResult(
        output: fmt"TIMEOUT after {timeout}s ({ms}ms)" & "\n\n" & output,
        isError: true)
    if endKind == weCancelled:
      return ToolResult(
        output: fmt"INTERRUPTED ({ms}ms)" & "\n\n" & output,
        isError: true)

    let tailKeep = min(TailKeep, outputLimit div 2)
    if output.len > outputLimit:
      let head = output[0 ..< (outputLimit - tailKeep)]
      let tail = output[^tailKeep .. ^1]
      output = head & "\n\n[... truncated " &
        $(output.len - outputLimit) & " bytes ...]\n\n" & tail

    var buf = fmt"exit_code: {exitCode}" & "\n"
    buf.add fmt"duration_ms: {ms}" & "\n\n"
    buf.add output

    ToolResult(output: buf, isError: exitCode != 0)

  (def, run)
