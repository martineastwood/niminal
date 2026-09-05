import std/[os, strutils, terminal]
import config, agent, session, models_dev, hooks
import ui/[console, tui, turn]

type
  CliArgs* = object
    help*: bool
    resumeLatest*: bool
    sessionId*: string
    ## Stay in the REPL after a CLI prompt (default: one-shot when prompt set).
    interactive*: bool
    prompt*: string
    error*: string

proc usageLine(): string =
  "Usage: niminal [options] [prompt…]"

proc parseCliArgs*(args: openArray[string]): CliArgs =
  ## Flags may precede the prompt. `--` ends flags. Unknown `-…` flags error
  ## unless they follow `--` or are already part of the prompt words.
  var i = 0
  var promptParts: seq[string]
  var sawPrompt = false
  var endFlags = false
  while i < args.len:
    let a = args[i]
    if endFlags or sawPrompt:
      promptParts.add a
      inc i
      continue
    case a
    of "--help", "-h":
      result.help = true
      return
    of "--resume":
      result.resumeLatest = true
    of "--session":
      if i + 1 >= args.len:
        result.error = "Usage: niminal --session ID"
        return
      result.sessionId = args[i + 1]
      inc i
    of "--interactive", "-i":
      result.interactive = true
    of "--":
      endFlags = true
    else:
      if a.startsWith("-"):
        result.error = "Unknown option: " & a & "\n" & usageLine()
        return
      sawPrompt = true
      promptParts.add a
    inc i
  result.prompt = promptParts.join(" ").strip

proc catalogStartupNote(): string =
  if not modelsDevCacheStale(): return ""
  echo "Refreshing model catalog…".color(cDim)
  if refreshModelsDevCache():
    "Model catalog updated."
  else:
    "Could not refresh model catalog; using cache."

proc printStartupBanner(agent: Agent, catalogNote: string) =
  echo "niminal — minimal coding agent".color(cBold)
  echo ("Provider: " & agent.config.provider & "  Model: " & agent.config.model).color(cDim)
  echo ("Workspace: " & agent.config.workspace).color(cDim)
  echo ("Session: " & agent.session.id).color(cDim)
  if catalogNote.len > 0:
    echo catalogNote.color(cDim)
  for warning in agent.extensionWarnings:
    echo ("extension: " & warning).color(cDim)
  for warning in agent.hookWarnings:
    echo ("hook: " & warning).color(cDim)

proc runOneShot(agent: var Agent, prompt: string, catalogNote = "") =
  ## Run a single turn from a CLI prompt, then exit.
  printStartupBanner(agent, catalogNote)
  let ui = consoleSink()
  defer: agent.fireSessionHooks(heSessionEnd)
  discard agent.processInput(prompt, ui)

proc runConsole(agent: var Agent, catalogNote = "", initialPrompt = "") =
  printStartupBanner(agent, catalogNote)
  echo "Type /help for commands.".color(cDim)

  let ui = consoleSink()
  defer: agent.fireSessionHooks(heSessionEnd)
  if initialPrompt.len > 0:
    discard agent.processInput(initialPrompt, ui)
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

proc runTUI(agent: var Agent, catalogNote = "", initialPrompt = "") =
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
  for warning in agent.extensionWarnings:
    tui.addLine("\e[2mextension: " & warning & "\e[0m")
  for warning in agent.hookWarnings:
    tui.addLine("\e[2mhook: " & warning & "\e[0m")
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

  if initialPrompt.len > 0:
    tui.addUserMessage(initialPrompt)
    tui.setBusy(true)
    tui.render()
    discard agent.processInput(initialPrompt, ui)
    tui.setBusy(false)
    tui.render()

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
  agent.fireSessionHooks(heSessionEnd)

proc runMain*() =
  let cli = parseCliArgs(commandLineParams())
  if cli.help:
    printHelp()
    echo "  --session ID     resume a session at startup"
    echo "  --resume         resume the latest session, if any"
    echo "  --interactive,-i keep the REPL after a CLI prompt"
    echo "  prompt…          run this as the first user message (one-shot unless -i)"
    return
  if cli.error.len > 0:
    stderr.writeLine cli.error
    quit(2)

  var sessionId = cli.sessionId
  let config = loadConfig(getCurrentDir())
  if cli.resumeLatest and sessionId.len == 0:
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

  agent.fireSessionHooks(heSessionStart)

  if cli.prompt.len > 0 and not cli.interactive:
    runOneShot(agent, cli.prompt, catalogNote)
    return

  if stdout.isatty:
    runTUI(agent, catalogNote, cli.prompt)
  else:
    runConsole(agent, catalogNote, cli.prompt)

when isMainModule:
  runMain()
