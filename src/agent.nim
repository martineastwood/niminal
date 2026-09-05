## Minimal synchronous agent loop.

import std/[json, strutils]
import config, session, compaction, instructions, skills, models_dev, commands
import workspace
import images
import extensions, hooks
import nimgent
import nimgent/[anthropic, openai]
import tools/[tool, read_tool, edit_tool, write_tool, bash_tool, search_tool]
import ui/turn

const baseSystemPrompt = """
You are a coding agent in the user's workspace. Use tools to inspect and
change the repo; do not only describe a plan.

Tools:
- read: file contents with line numbers and a version hash. Read a file before editing it.
- grep: PCRE regex search (plain text still works). Optional glob and subdirectory path.
- glob: list files matching a glob (e.g. **/*.nim).
- edit: unique old_text → new_text. Use replacements=[{old_text,new_text},…] for several hunks in one call. Pass expected_version from that read.
- write: create a file, or replace one only with overwrite=true. Prefer edit for existing files.
- bash: run commands in the workspace (tests, git). Prefer grep/glob over bash for finding files.
- read_skill: load a listed skill when it fits the task.

Rules:
- Stay in the workspace. Use relative paths. Do not invent file contents.
- Make the smallest change that solves the request.
- After edits, run the relevant tests or build. Report what you ran and what failed.
- Do not commit, push, or rewrite git history unless asked.
- Do not add unrelated files, docs, or refactors.
- If a tool fails, adjust and retry; never claim a change that did not apply.
- Be concise. Lead with the outcome.
"""

type
  Agent* = object
    config*: AgentConfig
    provider*: Provider
    session*: Session
    tools*: ToolRegistry
    hooks*: seq[Hook]
    ## Runtime thinking override; empty means use config.thinking.
    thinking*: string
    ## Startup warnings from extension discovery (invalid manifests, collisions).
    extensionWarnings*: seq[string]
    ## Startup warnings from hook discovery (invalid manifests).
    hookWarnings*: seq[string]

proc effectiveThinking*(agent: Agent): string =
  if agent.thinking.len > 0: agent.thinking else: agent.config.thinking

proc statusFooter*(agent: Agent): string =
  ## TUI status-bar text: model, usage, context fill, session id, thinking.
  var parts: seq[string] = @[]
  let (found, storedModel, usage) = agent.session.lastAssistant
  let model = if storedModel.len > 0: storedModel else: agent.config.model
  if model.len > 0:
    parts.add "\e[35m" & model & "\e[0m"
  if found:
    let labels = formatUsageLabels(usage)
    for label in labels:
      parts.add "\e[2m" & label & "\e[0m"
    let cost = formatUsageCost(agent.config.provider, model, usage)
    if cost.len > 0:
      parts.add "\e[2m" & cost & "\e[0m"
    let window = agent.config.effectiveContextWindow
    if labels.len > 0 and window > 0:
      let used = contextTokens(usage)
      if used > 0:
        let pct = min(100, used * 100 div window)
        let color =
          if pct >= 90: "\e[31m"
          elif pct >= 70: "\e[33m"
          else: "\e[2m"
        parts.add color & "ctx " & $pct & "%\e[0m"
  if agent.session.id.len > 0:
    parts.add "\e[2m#" & agent.session.id & "\e[0m"
  let level = thinkingStatus(agent.config, agent.thinking)
  if level == "off":
    parts.add "\e[2mthink:off\e[0m"
  elif level.len > 0:
    parts.add "\e[33mthink:" & level & "\e[0m"
  parts.join("  ")

proc attachProvider(agent: var Agent) =
  case agent.config.provider.toLowerAscii
  of "openrouter":
    agent.provider = makeOpenRouterProvider(agent.config.apiKey,
      agent.config.endpoint, agent.config.requestTimeout,
      agent.config.siteUrl, agent.config.siteName)
  of "openai":
    agent.provider = makeOpenAIProvider(agent.config.apiKey,
      agent.config.endpoint, agent.config.requestTimeout)
  of "anthropic":
    agent.provider = makeAnthropicProvider(agent.config.apiKey,
      agent.config.model, agent.config.endpoint, agent.config.requestTimeout)
  of "hyper":
    agent.provider = makeHyperProvider(agent.config.apiKey,
      agent.config.endpoint, agent.config.requestTimeout)
  else:
    raise newException(ValueError, "unsupported provider: " & agent.config.provider)

proc applyModel*(agent: var Agent, id: string, persist = true) =
  ## Set model for this process and persist to the write-target config.
  let keyed = keyedWiredProviders(agent.config)
  let (found, row) = findCatalogModel(id, keyed, agent.config.provider)
  agent.config.model = id
  if found and row.provider != agent.config.provider:
    agent.config.fillProvider(row.provider)
    agent.attachProvider()
  if persist:
    persistModel(agent.config)

proc modelPickerFrom*(agent: Agent): ModelPicker =
  ModelPicker(
    currentModel: agent.config.model,
    defaultModel: agent.config.defaultModel,
    currentProvider: agent.config.provider,
    keyedProviders: keyedWiredProviders(agent.config))

proc restoreSessionModel(agent: var Agent) =
  let (found, storedModel, _) = agent.session.lastAssistant
  if found and storedModel.len > 0:
    agent.applyModel(storedModel, persist = false)

proc reloadToolsAndHooks*(agent: var Agent) =
  ## Rescan external tools and hooks from disk (builtins stay the same set).
  var reg: ToolRegistry
  let ws = initWorkspace(agent.config.workspace)
  let read = makeReadTool(ws)
  let edit = makeEditTool(ws)
  let write = makeWriteTool(ws)
  let grep = makeGrepTool(ws)
  let glob = makeGlobTool(ws)
  let bash = makeBashTool(ws.root, agent.config.maxToolOutputBytes)
  let skill = makeSkillTool(agent.config.workspace)
  reg.register(read[0], read[1])
  reg.register(grep[0], grep[1])
  reg.register(glob[0], glob[1])
  reg.register(edit[0], edit[1])
  reg.register(write[0], write[1])
  reg.register(bash[0], bash[1])
  reg.register(skill[0], skill[1])
  agent.extensionWarnings = reg.registerExtensions(
    agent.config.workspace, agent.config.maxToolOutputBytes)
  agent.tools = reg
  let discovered = discoverHooks(agent.config.workspace)
  agent.hooks = discovered.hooks
  agent.hookWarnings = discovered.warnings

proc emitDiscoveryWarnings(agent: Agent, ui: TurnSink) =
  for warning in agent.extensionWarnings:
    ui.emit(mlWarn, "extension: " & warning)
  for warning in agent.hookWarnings:
    ui.emit(mlWarn, "hook: " & warning)

proc initAgent*(config: AgentConfig, sessionId = ""): Agent =
  result.config = config
  result.thinking = config.thinking
  result.attachProvider()
  result.session = loadSession(config.sessionDir, sessionId, config.workspace)
  if sessionId.len > 0:
    result.restoreSessionModel()
  result.reloadToolsAndHooks()

proc buildRequest*(agent: Agent): ProviderRequest =
  let opts = providerOptions(agent.config, agent.thinking)
  var maxTok = agent.config.maxTokens
  if "thinking" in opts:
    let budget = opts["thinking"]["budget_tokens"].getInt
    if maxTok <= budget:
      maxTok = budget + 4096
  result = ProviderRequest(
    model: agent.config.model,
    sessionId: agent.session.id,
    system: @[baseSystemPrompt],
    messages: agent.session.messagesForModel,
    tools: agent.tools.definitions,
    maxTokens: maxTok,
    options: opts
  )
  let projectInstructions = loadProjectInstructions(agent.config.workspace)
  if projectInstructions.len > 0:
    result.system.add projectInstructions
  let availableSkills = skillMetadataPrompt(agent.config.workspace)
  if availableSkills.len > 0:
    result.system.add availableSkills
  if lookupAcceptsImages(agent.config.provider, agent.config.model):
    result.messages = hydrateMessages(initWorkspace(agent.config.workspace),
      result.messages)
  else:
    result.messages = dropImages(result.messages)

proc setThinking*(agent: var Agent, value: string): string =
  try:
    agent.thinking = normalizeThinking(value)
    agent.config.thinking = agent.thinking
    persistModel(agent.config)
    if agent.thinking.len == 0:
      result = "(provider default)"
    else:
      result = agent.thinking
  except ValueError as e:
    result = "ERROR: " & e.msg

proc compactionPoll(ui: TurnSink): StreamCallback =
  proc (_: StreamEvent): bool =
    ui.poll()
    not ui.wasInterrupted()

proc emitHookWarnings(ui: TurnSink, warnings: openArray[string]) =
  for w in warnings:
    ui.emit(mlWarn, "hook: " & w)

proc warnHooks(ui: TurnSink, warnings: openArray[string]) =
  if not ui.emit.isNil:
    emitHookWarnings(ui, warnings)
  else:
    for w in warnings:
      stderr.writeLine "hook: " & w

proc runCompaction*(agent: var Agent, instruction = "",
                    onEvent: StreamCallback = nil,
                    ui: TurnSink = default(TurnSink)): CompactionResult =
  let tokensBefore = estimatedContextTokens(agent.session)
  var instruction = instruction
  let pre = runHooks(agent.hooks, hePreCompact,
    preCompactPayload(agent.session.id, agent.config.workspace, instruction,
      tokensBefore),
    agent.config.workspace, maxOutputBytes = agent.config.maxToolOutputBytes)
  warnHooks(ui, pre.warnings)
  if not pre.allowed:
    result.message = if pre.reason.len > 0: pre.reason else: "blocked by hook"
    return
  if pre.instruction.len > 0:
    if instruction.len > 0:
      instruction = instruction & "\n" & pre.instruction
    else:
      instruction = pre.instruction
  try:
    result = prepareAndCompact(
      agent.session,
      agent.provider,
      agent.config.model,
      agent.config.keepRecentTokens,
      instruction,
      onEvent)
  except CatchableError as e:
    result.message = "Compaction failed: " & e.msg
  let post = runHooks(agent.hooks, hePostCompact,
    postCompactPayload(agent.session.id, agent.config.workspace,
      result.didCompact, result.summary, result.firstKeptIndex,
      result.tokensBefore, result.message),
    agent.config.workspace, maxOutputBytes = agent.config.maxToolOutputBytes)
  warnHooks(ui, post.warnings)

proc maybeAutoCompact*(agent: var Agent,
                       onEvent: StreamCallback = nil,
                       ui: TurnSink = default(TurnSink)): CompactionResult =
  if not agent.config.compactionEnabled:
    result.message = "auto-compaction disabled"
    return
  let window = agent.config.effectiveContextWindow
  if not shouldCompact(agent.session, window, agent.config.reserveTokens):
    result.message = "below threshold"
    return
  result = agent.runCompaction(onEvent = onEvent, ui = ui)

proc fireSessionHooks*(agent: var Agent, event: HookEvent, ui: TurnSink) =
  let payload = sessionPayload(agent.session.id, agent.config.workspace)
  let outcome = runHooks(agent.hooks, event, payload, agent.config.workspace,
    maxOutputBytes = agent.config.maxToolOutputBytes)
  emitHookWarnings(ui, outcome.warnings)

proc fireSessionHooks*(agent: var Agent, event: HookEvent) =
  ## No UI: write fail-open warnings to stderr.
  let payload = sessionPayload(agent.session.id, agent.config.workspace)
  let outcome = runHooks(agent.hooks, event, payload, agent.config.workspace,
    maxOutputBytes = agent.config.maxToolOutputBytes)
  for w in outcome.warnings:
    stderr.writeLine "hook: " & w

proc fireTurnHooks(agent: var Agent, event: HookEvent, ui: TurnSink,
                   interrupted = false) =
  let payload = turnPayload(agent.session.id, agent.config.workspace, interrupted)
  let outcome = runHooks(agent.hooks, event, payload, agent.config.workspace,
    maxOutputBytes = agent.config.maxToolOutputBytes)
  emitHookWarnings(ui, outcome.warnings)

proc switchSession(agent: var Agent, next: Session, ui: TurnSink) =
  ## session_end on the old transcript, rescan tools/hooks, then session_start.
  agent.fireSessionHooks(heSessionEnd, ui)
  agent.reloadToolsAndHooks()
  agent.emitDiscoveryWarnings(ui)
  agent.session = next
  agent.fireSessionHooks(heSessionStart, ui)

proc applySlash(agent: var Agent, cmd: SlashCommand, ui: TurnSink) =
  ## Execute a parsed builtin. Caller has already filtered slNone/slSkill/slError/slQuit.
  case cmd.kind
  of slHelp:
    ui.emit(mlPlain, helpText().strip)
  of slModel:
    if cmd.arg.len == 0:
      ui.emit(mlPlain, agent.config.model)
    else:
      agent.applyModel(cmd.arg)
      ui.emit(mlPlain, agent.config.provider & "  " & agent.config.model)
      ui.onChange()
  of slThinking:
    if cmd.arg.len == 0:
      let level = thinkingStatus(agent.config, agent.thinking)
      ui.emit(mlPlain, if level.len == 0: "(provider default)" else: level)
    else:
      ui.emit(mlPlain, agent.setThinking(cmd.arg))
      ui.onChange()
  of slProvider:
    ui.emit(mlPlain, agent.provider.name)
  of slModelsRefresh:
    ui.emit(mlWarn, "Refreshing model metadata…")
    ui.render()
    if refreshModelsDevCache():
      ui.emit(mlOk, "Model metadata refreshed.")
      ui.onChange()
    else:
      ui.emit(mlError, "Could not refresh model metadata; using existing cache.")
  of slNew:
    let next = loadSession(agent.config.sessionDir, workspace = agent.config.workspace)
    agent.switchSession(next, ui)
    if not ui.showSession.isNil:
      ui.showSession(agent.session)
    ui.onChange()
  of slCompact:
    ui.emit(mlWarn, "Compacting…")
    ui.render()
    let res = agent.runCompaction(cmd.arg, compactionPoll(ui), ui)
    if res.didCompact: ui.emit(mlOk, res.message)
    else: ui.emit(mlDim, res.message)
    ui.onChange()
  of slSession:
    ui.emit(mlPlain, "Session: " & agent.session.id)
    if agent.session.name.len > 0:
      ui.emit(mlPlain, "Name: " & agent.session.name)
    ui.emit(mlPlain, "Events: " & $agent.session.events.len)
    ui.emit(mlPlain, "File: " & agent.session.path)
    if agent.session.workspace.len > 0:
      ui.emit(mlPlain, "Workspace: " & agent.session.workspace)
    let think = if agent.effectiveThinking.len == 0: "(default)"
                else: agent.effectiveThinking
    ui.emit(mlPlain, "Thinking: " & think)
  of slResume:
    if cmd.arg.len == 0:
      let sessions = listSessions(agent.config.sessionDir, agent.config.workspace)
      if sessions.len == 0:
        ui.emit(mlPlain, "No saved sessions in this workspace.")
      else:
        ui.emit(mlPlain, "Sessions (newest first):")
        for info in sessions:
          ui.emit(mlPlain, "  " & sessionListLine(info, agent.session.id))
        if sessions.len == sessionListLimit:
          ui.emit(mlDim, "Showing newest " & $sessionListLimit & ".")
    else:
      let (ok, sess, err) = tryLoadSession(agent.config.sessionDir, cmd.arg)
      if not ok:
        ui.emit(mlPlain, err)
      else:
        agent.switchSession(sess, ui)
        agent.restoreSessionModel()
        if sess.workspace.len > 0 and sess.workspace != agent.config.workspace:
          ui.emit(mlWarn, "This session was started in " & sess.workspace)
        if not ui.showSession.isNil:
          ui.showSession(agent.session)
        ui.onChange()
  of slName:
    if cmd.arg.len == 0:
      ui.emit(mlPlain, if agent.session.name.len == 0: "(unnamed)"
                       else: agent.session.name)
    else:
      agent.session.setName(cmd.arg)
      ui.emit(mlOk, agent.session.name)
      ui.onChange()
  of slReload:
    agent.reloadToolsAndHooks()
    agent.emitDiscoveryWarnings(ui)
    ui.emit(mlOk, "Reloaded tools and hooks.")
    ui.onChange()
  of slQuit, slNone, slError, slSkill:
    discard

proc emitAutoCompact(ui: TurnSink, res: CompactionResult) =
  if not res.didCompact: return
  ui.emit(mlWarn, "Auto-compacted context")
  ui.emit(mlDim, res.message)
  ui.render()

proc retryAfterOverflow(agent: var Agent, e: ref ProviderError,
                        overflowRetried: var bool, ui: TurnSink): bool =
  if not e.overflow or overflowRetried:
    return false
  overflowRetried = true
  ui.emit(mlWarn, "Context overflow — compacting and retrying…")
  ui.render()
  let res = agent.runCompaction("Prioritize recovering from context overflow.",
    compactionPoll(ui), ui)
  ui.emit(mlDim, res.message)
  res.didCompact

proc persistInterruptedToolResults(session: var Session,
                                   calls: openArray[ContentBlock],
                                   firstPending: int) =
  ## Keep the next provider request structurally valid after cancellation.
  if firstPending >= calls.len:
    return
  for i in firstPending ..< calls.len:
    session.addToolResult(calls[i], "Interrupted before tool execution.", true)

proc runTurn*(agent: var Agent, ui: TurnSink) =
  agent.fireTurnHooks(heTurnStart, ui)
  var overflowRetried = false
  while true:
    emitAutoCompact(ui, agent.maybeAutoCompact(compactionPoll(ui), ui))
    let request = agent.buildRequest()
    var response: ProviderResponse
    try:
      response = ui.generate(agent.provider, request)
    except ProviderError as e:
      if retryAfterOverflow(agent, e, overflowRetried, ui):
        continue
      ui.emit(mlError, e.msg)
      agent.fireTurnHooks(heTurnEnd, ui)
      return
    except CatchableError as e:
      # Overflow is only flagged on ProviderError; other failures surface as-is.
      ui.emit(mlError, e.msg)
      agent.fireTurnHooks(heTurnEnd, ui)
      return

    if ui.wasInterrupted():
      agent.fireTurnHooks(heTurnEnd, ui, interrupted = true)
      return

    overflowRetried = false
    agent.session.addAssistantResponse(response)
    let calls = response.toolCalls()
    let final = calls.len == 0
    ui.commitGenerate(response, final)
    if final:
      agent.fireTurnHooks(heTurnEnd, ui)
      return

    for i in 0 ..< calls.len:
      let call = calls[i]
      ui.toolStart(call)
      ui.poll()
      if ui.wasInterrupted():
        agent.session.persistInterruptedToolResults(calls, i)
        ui.noteInterrupted()
        agent.fireTurnHooks(heTurnEnd, ui, interrupted = true)
        return
      let bad = invalidToolCall(call)
      var toolResult: ToolResult
      if bad.len > 0:
        toolResult = ToolResult(output: bad, isError: true)
      else:
        var args = if call.input.isNil: newJObject() else: call.input
        let pre = runHooks(agent.hooks, hePreToolCall,
          preToolPayload(call.name, args), agent.config.workspace,
          call.name, agent.config.maxToolOutputBytes)
        emitHookWarnings(ui, pre.warnings)
        if not pre.allowed:
          toolResult = ToolResult(output: pre.reason, isError: true)
        else:
          if not pre.arguments.isNil:
            args = pre.arguments
          toolResult = agent.tools.execute(call.name, args, proc (): bool =
            ui.poll()
            ui.wasInterrupted())
          let post = runHooks(agent.hooks, hePostToolCall,
            postToolPayload(call.name, args, toolResult.output,
              toolResult.isError),
            agent.config.workspace, call.name, agent.config.maxToolOutputBytes)
          emitHookWarnings(ui, post.warnings)
          if post.hasOutput:
            toolResult.output = post.output
          if post.hasIsError:
            toolResult.isError = post.isError
      agent.session.addToolResult(call, toolResult.output, toolResult.isError,
        toolResult.images)
      ui.toolResult(toolResult.output, toolResult.isError)
      ui.poll()
      if ui.wasInterrupted():
        agent.session.persistInterruptedToolResults(calls, i + 1)
        ui.noteInterrupted()
        agent.fireTurnHooks(heTurnEnd, ui, interrupted = true)
        return

proc processInput*(agent: var Agent, input: string, ui: TurnSink): bool =
  ## Returns false when the caller should exit.
  let command = input.strip
  let cmd = parseSlash(command, agent.config.workspace)
  case cmd.kind
  of slNone:
    if command.len == 0: return true
    agent.session.addUserMessage(expandUserContent(agent.config.workspace, input))
    runTurn(agent, ui)
    true
  of slSkill:
    let text = expandSkill(agent.config.workspace, cmd)
    let body = if text.len == 0: input else: text
    agent.session.addUserMessage(expandUserContent(agent.config.workspace, body))
    runTurn(agent, ui)
    true
  of slError:
    ui.emit(mlError, cmd.error)
    true
  of slQuit:
    false
  of slHelp, slModel, slModelsRefresh, slThinking, slProvider, slSession,
     slNew, slCompact, slResume, slReload, slName:
    applySlash(agent, cmd, ui)
    true

proc processInput*(agent: var Agent, input: string): bool =
  ## Console-default overload (tests and non-TTY callers).
  processInput(agent, input, consoleSink())
