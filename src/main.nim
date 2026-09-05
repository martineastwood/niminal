import std/[os, strutils, terminal]
import config, agent, session, models_dev
import ui/[console, tui, turn]

proc catalogStartupNote(): string =
  if not modelsDevCacheStale(): return ""
  echo "Refreshing model catalog…".color(cDim)
  if refreshModelsDevCache():
    "Model catalog updated."
  else:
    "Could not refresh model catalog; using cache."

proc runConsole(agent: var Agent, catalogNote = "") =
  echo "niminal — minimal coding agent".color(cBold)
  echo ("Provider: " & agent.config.provider & "  Model: " & agent.config.model).color(cDim)
  echo ("Workspace: " & agent.config.workspace).color(cDim)
  echo ("Session: " & agent.session.id).color(cDim)
  if catalogNote.len > 0:
    echo catalogNote.color(cDim)
  echo "Type /help for commands.".color(cDim)

  let ui = consoleSink()
  while true:
    printPrompt()
    try:
      let input = stdin.readLine()
      if not agent.processInput(input, ui):
        break
    except IOError:
      break
    except CatchableError as e:
      stderr.writeLine "ERROR: " & e.msg

proc runTUI(agent: var Agent, catalogNote = "") =
  var tui = initTUI(agent.config.workspace, agent.config.sessionDir)
  defer: tui.shutdown()
  tui.modelPicker = modelPickerFrom(agent)

  tui.addLine("\e[1;93mniminal — minimal coding agent\e[0m")
  tui.addLine("\e[2mProvider: " & agent.config.provider &
    "  Model: " & agent.config.model & "\e[0m")
  tui.addLine("\e[2mWorkspace: " & agent.config.workspace & "\e[0m")
  tui.addLine("\e[2mSession: " & agent.session.id & "\e[0m")
  if catalogNote.len > 0:
    tui.addLine("\e[2m" & catalogNote & "\e[0m")
  if agent.session.events.len > 0:
    tui.addLine("")
    tui.replaySession(agent.session)
  else:
    tui.addLine("\e[2mType /help for commands.\e[0m")
    tui.addLine("")

  tui.setFooter(agent.statusFooter)
  tui.render()

  let agentPtr = addr agent
  let ui = tuiSink(addr tui, proc (): string =
    tui.modelPicker = modelPickerFrom(agentPtr[])
    agentPtr[].statusFooter)
  while not tui.shouldExit:
    let input = tui.readLineBlocking()
    if tui.shouldExit: break
    if input.strip.len == 0: continue
    tui.addUserMessage(input)
    tui.setBusy(true)
    tui.render()
    let keepGoing = agent.processInput(input, ui)
    tui.setBusy(false)
    tui.render()
    if not keepGoing: break

proc runMain*() =
  let args = commandLineParams()
  if args.len > 0 and args[0] == "--help":
    printHelp()
    echo "  --session ID  resume a session at startup"
    echo "  --resume      resume the latest session, if any"
    return
  var sessionId = ""
  var resumeLatest = false
  if args.len > 0:
    if args.len == 1 and args[0] == "--resume":
      resumeLatest = true
    elif args.len == 2 and args[0] == "--session":
      sessionId = args[1]
    else:
      stderr.writeLine "Usage: niminal [--session ID | --resume]"
      quit(2)
  let config = loadConfig(getCurrentDir())
  if resumeLatest:
    let ids = listSessionIds(config.sessionDir, config.workspace)
    if ids.len > 0:
      sessionId = ids[0]
  let catalogNote = catalogStartupNote()
  var agent: Agent
  try:
    agent = initAgent(config, sessionId)
  except CatchableError as e:
    stderr.writeLine "STARTUP_FAILED"
    stderr.writeLine e.msg
    quit(1)

  if stdout.isatty:
    runTUI(agent, catalogNote)
  else:
    runConsole(agent, catalogNote)

when isMainModule:
  runMain()
