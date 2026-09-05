import std/[json, os, strutils, times, unittest]
when defined(posix):
  import posix
import ../src/workspace
import ../src/images
import ../src/session
import ../src/config
import ../src/agent
import ../src/ui/markdown
import ../src/ui/ansi
import ../src/ui/diff
import ../src/ui/turn
import ../src/ui/tui
import ../src/ui/input
import ../src/models_dev
import ../src/compaction
import ../src/instructions
import ../src/skills
import ../src/commands
import nimgent
import nimgent/[anthropic, openrouter]
import ../src/tools/[tool, read_tool, edit_tool, write_tool, bash_tool]

proc freshDir(): string =
  result = getTempDir() / ("niminal-test-" & $getCurrentProcessId() & "-" &
    $int(epochTime() * 1_000_000))
  createDir(result)

proc invoke(pair: (ToolDefinition, ToolProc), input: JsonNode): ToolResult =
  pair[1](input)

type
  TestProvider = ref object of Provider
    responses: seq[ProviderResponse]
    callCount: int

method generate(provider: TestProvider,
                request: ProviderRequest): ProviderResponse =
  result = provider.responses[min(provider.callCount, provider.responses.high)]
  inc provider.callCount

suite "workspace and file tools":
  test "read returns numbered lines and version":
    let root = freshDir()
    defer: removeDir(root)
    writeFile(root / "sample.txt", "one\ntwo\nthree\n")
    let result = invoke(makeReadTool(initWorkspace(root)),
      %*{"path": "sample.txt", "start_line": 2, "end_line": 2})
    check not result.isError
    check "version:" in result.output
    check "2 | two" in result.output

  test "read rejects traversal":
    let root = freshDir()
    defer: removeDir(root)
    let result = invoke(makeReadTool(initWorkspace(root)), %*{"path": "../escape"})
    check result.isError
    check "outside the workspace" in result.output

  test "edit performs one exact replacement":
    let root = freshDir()
    defer: removeDir(root)
    let path = root / "sample.txt"
    writeFile(path, "before\nkeep\n")
    let version = hashContent(readFile(path))
    let result = invoke(makeEditTool(initWorkspace(root)), %*{
      "path": "sample.txt",
      "old_text": "before",
      "new_text": "after",
      "expected_version": version
    })
    check not result.isError
    check readFile(path) == "after\nkeep\n"

  test "edit rejects ambiguous and stale replacements":
    let root = freshDir()
    defer: removeDir(root)
    let path = root / "sample.txt"
    writeFile(path, "same\nsame\n")
    let edit = makeEditTool(initWorkspace(root))
    check invoke(edit, %*{"path": "sample.txt", "old_text": "same",
      "new_text": "new"}).isError
    let stale = invoke(edit, %*{"path": "sample.txt", "old_text": "same\nsame\n",
      "new_text": "new", "expected_version": "stale"})
    check stale.isError
    check "version changed" in stale.output

  test "write creates and protects existing files":
    let root = freshDir()
    defer: removeDir(root)
    let write = makeWriteTool(initWorkspace(root))
    check not invoke(write, %*{"path": "new.txt", "content": "hello"}).isError
    check invoke(write, %*{"path": "new.txt", "content": "no"}).isError
    check not invoke(write, %*{"path": "new.txt", "content": "yes",
      "overwrite": true}).isError
    check readFile(root / "new.txt") == "yes"

suite "bash tool":
  test "captures output and exit code":
    let root = freshDir()
    defer: removeDir(root)
    let bash = makeBashTool(root)
    let success = invoke(bash, %*{"command": "printf hello"})
    check not success.isError
    check "exit_code: 0" in success.output
    check "hello" in success.output
    let failure = invoke(bash, %*{"command": "printf error >&2; exit 3"})
    check failure.isError
    check "exit_code: 3" in failure.output
    check "stderr:" in failure.output

  test "enforces timeout":
    let root = freshDir()
    defer: removeDir(root)
    let result = invoke(makeBashTool(root),
      %*{"command": "sleep 2", "timeout_seconds": 1})
    check result.isError
    check "TIMEOUT" in result.output

  test "cancels a running command":
    let root = freshDir()
    defer: removeDir(root)
    var reg: ToolRegistry
    let bash = makeBashTool(root)
    reg.register(bash[0], bash[1])
    let t0 = epochTime()
    let result = reg.execute("bash", %*{"command": "sleep 5"},
      proc (): bool = true)
    check result.isError
    check "INTERRUPTED" in result.output
    check epochTime() - t0 < 2.0

  test "cancel kills the process group":
    let root = freshDir()
    defer: removeDir(root)
    var reg: ToolRegistry
    let bash = makeBashTool(root)
    reg.register(bash[0], bash[1])
    let pidPath = root / "pid"
    let result = reg.execute("bash",
      %*{"command": "sleep 8 &\necho $! > pid\nwait", "timeout_seconds": 3},
      proc (): bool =
        # Wait is event-driven; this test's cancel signal is a file, not an fd.
        for _ in 0 .. 50:
          if fileExists(pidPath): return true
          sleep(10)
        false)
    check result.isError
    check "INTERRUPTED" in result.output
    let child = readFile(pidPath).strip.parseInt
    sleep(50)
    when defined(posix):
      check posix.kill(Pid(child), 0) != 0

suite "session":
  test "JSONL round trip and partial final line recovery":
    let root = freshDir()
    defer: removeDir(root)
    let path = root / "session.jsonl"
    var original = initSession(path, "test")
    original.addUserMessage("hello")
    original.addAssistantResponse(ProviderResponse(content: @[text("world")]))
    check original.events.len == 2
    var file = open(path, fmAppend)
    file.write("{\"type\":\"assistant\"")
    file.close()
    let recovered = initSession(path, "test")
    check recovered.events.len == 2
    check recovered.messages.len == 2
    check recovered.messages[0].content[0].text == "hello"
    check recovered.messages[1].content[0].text == "world"

  test "lists session ids newest first":
    let root = freshDir()
    defer: removeDir(root)
    writeFile(root / "older.jsonl", "")
    writeFile(root / "newer.jsonl", "")
    writeFile(root / "skip.txt", "")
    setLastModificationTime(root / "older.jsonl", fromUnix(1_000))
    setLastModificationTime(root / "newer.jsonl", fromUnix(2_000))
    let ids = listSessionIds(root)
    check ids == @["newer", "older"]
    check listSessionIds(root / "missing").len == 0

  test "lists sessions with first-user preview and relative age":
    let root = freshDir()
    defer: removeDir(root)
    var older = initSession(root / "older.jsonl", "older")
    older.addUserMessage("fix the failing parser test\nand more")
    var newer = initSession(root / "newer.jsonl", "newer")
    newer.addUserMessage("a".repeat(80))
    writeFile(root / "empty.jsonl", "")
    setLastModificationTime(root / "older.jsonl", fromUnix(1_000))
    setLastModificationTime(root / "newer.jsonl", fromUnix(2_000))
    setLastModificationTime(root / "empty.jsonl", fromUnix(1_500))
    let infos = listSessions(root)
    check infos.len == 3
    check infos[0].id == "newer"
    check infos[0].preview.startsWith("aaa")
    check infos[0].preview.endsWith("…")
    check infos[1].id == "empty"
    check infos[1].preview == "(empty)"
    check infos[2].id == "older"
    check infos[2].preview == "fix the failing parser test"
    let now = fromUnix(1_000 + 3600)
    check relativeAge(fromUnix(1_000), now) == "1h ago"
    check relativeAge(fromUnix(1_000), fromUnix(1_030)) == "just now"
    check relativeAge(fromUnix(1_000), fromUnix(1_000 + 90)) == "1m ago"
    check relativeAge(fromUnix(1_000), fromUnix(1_000 + 86400)) == "1d ago"
    let line = sessionListLine(infos[2], "older", now)
    check "1h ago" in line
    check "fix the failing parser test" in line
    check "#older" in line
    check "(current)" in line
    check peekSession(root, "older").preview == "fix the failing parser test"

  test "session name round-trips and labels the picker":
    let root = freshDir()
    defer: removeDir(root)
    var sess = initSession(root / "named.jsonl", "named")
    sess.addUserMessage("the first user prompt is long")
    sess.setName("Fix parser")
    let reloaded = initSession(root / "named.jsonl", "named")
    check reloaded.name == "Fix parser"
    check reloaded.events[^1].kind == sekName
    let info = peekSession(root, "named")
    check info.name == "Fix parser"
    check info.preview == "the first user prompt is long"
    check "Fix parser" in sessionLabel(info)
    check "the first user prompt is long" notin sessionLabel(info)

  test "workspace header round trip and listing filter":
    let root = freshDir()
    defer: removeDir(root)
    var here = initSession(root / "here.jsonl", "here")
    here.workspace = "/proj/here"
    here.addUserMessage("this repo")
    var there = initSession(root / "there.jsonl", "there")
    there.workspace = "/proj/there"
    there.addUserMessage("other repo")
    var legacy = initSession(root / "legacy.jsonl", "legacy")
    legacy.addUserMessage("old file")
    setLastModificationTime(root / "there.jsonl", fromUnix(3_000))
    setLastModificationTime(root / "here.jsonl", fromUnix(2_000))
    setLastModificationTime(root / "legacy.jsonl", fromUnix(1_000))
    let reloaded = initSession(root / "here.jsonl", "here")
    check reloaded.workspace == "/proj/here"
    check reloaded.events.len == 1
    let raw = readFile(root / "here.jsonl")
    check raw.startsWith("{\"type\":\"session\"")
    check listSessionIds(root, "/proj/here") == @["here", "legacy"]
    check listSessionIds(root, "/proj/there") == @["there", "legacy"]
    let infos = listSessions(root, "/proj/here")
    check infos.len == 2
    check infos[0].id == "here"
    check infos[0].workspace == "/proj/here"
    check infos[1].id == "legacy"
    check peekSession(root, "there").workspace == "/proj/there"

  test "session picker caps at newest 20":
    let root = freshDir()
    defer: removeDir(root)
    for i in 0 ..< sessionListLimit + 3:
      let id = "s" & $i
      writeFile(root / (id & ".jsonl"), "")
      setLastModificationTime(root / (id & ".jsonl"), fromUnix(1_000 + i))
    let infos = listSessions(root)
    check infos.len == sessionListLimit
    check infos[0].id == "s" & $(sessionListLimit + 2)
    check listSessionIds(root).len == sessionListLimit + 3

suite "OpenRouter provider":
  test "configuration defaults to OpenRouter":
    let root = freshDir()
    defer: removeDir(root)
    let config = loadConfig(root, root / "config.json")
    check config.provider == "openrouter"
    check config.model == "deepseek/deepseek-v4-flash-0731"
    check config.apiKeyEnv == "OPENROUTER_API_KEY"
    check config.endpoint == "https://openrouter.ai/api/v1/chat/completions"
    check niminalConfigDir().extractFilename == ".niminal"

suite "persistent agent sessions":
  test "new session files can be resumed":
    let root = freshDir()
    defer: removeDir(root)
    var config = loadConfig(root, root / "config.json")
    config.sessionDir = root / "sessions"
    var agent = initAgent(config)
    check agent.session.workspace == config.workspace
    let firstId = agent.session.id
    agent.session.addUserMessage("keep this")
    check fileExists(config.sessionDir / (firstId & ".jsonl"))
    check "\"type\":\"session\"" in readFile(config.sessionDir / (firstId & ".jsonl"))

    agent.session = initSession(config.sessionDir / (firstId & ".jsonl"), firstId)
    check agent.processInput("/new")
    let secondId = agent.session.id
    check secondId != firstId
    check agent.processInput("/resume " & firstId)
    check agent.session.id == firstId
    check agent.session.events.len == 1
    check agent.session.messages[0].content[0].text == "keep this"
    check agent.processInput("/resume")
    check agent.processInput("/name Fix parser")
    check agent.session.name == "Fix parser"
    check agent.processInput("/model other/model")
    check agent.config.model == "other/model"

  test "assistant usage is restored into status text":
    let root = freshDir()
    defer: removeDir(root)
    let path = root / "sess.jsonl"
    var session = initSession(path, "status1")
    var usage = Usage(inputTokens: 10, outputTokens: 4, cacheReadTokens: 8,
      cacheReported: true)
    session.addAssistantResponse(ProviderResponse(
      model: "test/model", usage: usage,
      content: @[text("hi")]))
    let reloaded = initSession(path, "status1")
    let (found, model, got) = reloaded.lastAssistant
    check found
    check model == "test/model"
    check got.inputTokens == 10
    check got.outputTokens == 4
    check got.cacheReadTokens == 8
    var agent = Agent(config: AgentConfig(model: "fallback", contextWindow: 100),
                      session: reloaded)
    let status = agent.statusFooter
    check "test/model" in status
    check "↑10" in status
    check "↓4" in status
    check "R8" in status
    check "ctx 10%" in status
    check "#status1" in status

  test "context percent uses anthropic-style split totals":
    var usage = Usage(inputTokens: 100, outputTokens: 1, cacheReadTokens: 900,
      cacheReported: true)
    check contextTokens(usage) == 1000
    var session = initSession()
    session.addAssistantResponse(ProviderResponse(
      model: "claude", usage: usage, content: @[text("x")]))
    var agent = Agent(config: AgentConfig(model: "claude", contextWindow: 2000),
                      session: session)
    check "ctx 50%" in agent.statusFooter

  test "resume restores last model and warns on foreign workspace":
    let root = freshDir()
    defer: removeDir(root)
    var config = loadConfig(root, root / "config.json")
    config.sessionDir = root / "sessions"
    createDir(config.sessionDir)
    var sess = initSession(config.sessionDir / "chat1.jsonl", "chat1")
    sess.workspace = "/other/project"
    sess.addUserMessage("hello")
    sess.addAssistantResponse(ProviderResponse(
      model: "kept/model", content: @[text("hi")]))
    var agent = initAgent(config, "chat1")
    check agent.config.model == "kept/model"
    check agent.session.workspace == "/other/project"
    agent.config.model = "fallback/model"
    var warns: seq[string] = @[]
    proc captureEmit(level: MsgLevel, text: string) =
      if level == mlWarn: warns.add text
    let ui = TurnSink(
      emit: captureEmit,
      render: proc() = discard,
      onChange: proc() = discard,
      commitGenerate: proc(response: ProviderResponse, final: bool) = discard,
      toolStart: proc(call: ContentBlock) = discard,
      toolResult: proc(output: string, isError: bool) = discard,
      poll: proc() = discard,
      wasInterrupted: proc(): bool = false,
      noteInterrupted: proc() = discard,
      generate: proc(provider: Provider,
                     request: ProviderRequest): ProviderResponse =
        provider.generate(request)
    )
    check agent.processInput("/resume chat1", ui)
    check agent.config.model == "kept/model"
    check warns.len == 1
    check "/other/project" in warns[0]

suite "agent turn persistence":
  test "interrupted tool rounds get synthetic results":
    var config = loadConfig()
    config.contextWindow = 1_000_000
    config.compactionEnabled = false
    let provider = TestProvider(
      name: "test",
      responses: @[
        ProviderResponse(content: @[
          text("working"),
          toolUse("one", "read", %*{"path": "one.txt"}),
          toolUse("two", "read", %*{"path": "two.txt"})
        ]),
        ProviderResponse(content: @[text("recovered")])
      ])
    var agent = Agent(config: config, provider: provider, session: initSession())
    agent.session.addUserMessage("inspect both files")
    var polls = 0
    let ui = TurnSink(
      emit: proc(level: MsgLevel, text: string) = discard,
      render: proc() = discard,
      onChange: proc() = discard,
      commitGenerate: proc(response: ProviderResponse, final: bool) = discard,
      toolStart: proc(call: ContentBlock) = discard,
      toolResult: proc(output: string, isError: bool) = discard,
      poll: proc() = (inc polls),
      wasInterrupted: proc(): bool = polls > 0,
      noteInterrupted: proc() = discard,
      generate: proc(provider: Provider,
                     request: ProviderRequest): ProviderResponse =
        provider.generate(request)
    )
    agent.runTurn(ui)

    check agent.session.events.len == 4
    check agent.session.events[2].kind == sekToolResult
    check agent.session.events[3].kind == sekToolResult
    check agent.session.events[2].toolError
    check agent.session.events[3].toolError
    check agent.session.messages[2].content.len == 2
    check agent.session.messages[2].content[0].toolUseId == "one"
    check agent.session.messages[2].content[1].toolUseId == "two"

suite "slash commands":
  test "suggests commands and validates arguments":
    check "/model [name]" in commandSuggestions("/mo")
    check "/models refresh" in commandSuggestions("/mo")
    check "/thinking high" in commandSuggestions("/thinking ")
    check parseSlash("/help").kind == slHelp
    check parseSlash("hello").kind == slNone
    check parseSlash("/thinking high").kind == slThinking
    check parseSlash("/thinking high").arg == "high"
    check parseSlash("/models refresh").kind == slModelsRefresh
    check parseSlash("/model refresh").kind == slError
    check parseSlash("/model").kind == slModel
    check parseSlash("/model").arg.len == 0
    check parseSlash("/model anthropic/claude-sonnet-4").kind == slModel
    check parseSlash("/model anthropic/claude-sonnet-4").arg == "anthropic/claude-sonnet-4"
    check parseSlash("/resume").kind == slResume
    check parseSlash("/resume").arg.len == 0
    check parseSlash("/resume abc").kind == slResume
    check parseSlash("/resume abc").arg == "abc"
    check parseSlash("/name").kind == slName
    check parseSlash("/name").arg.len == 0
    check parseSlash("/name Fix parser").kind == slName
    check parseSlash("/name Fix parser").arg == "Fix parser"
    check resumeOpensPicker("/resume")
    check not resumeOpensPicker("/resume abc")
    check not resumeOpensPicker("hello")
    check commandError("/models refresh") == ""
    check "did you mean /models refresh" in commandError("/model refresh")
    check "Unknown command" in commandError("/definitely-not-a-command")
    check "Invalid thinking level" in commandError("/thinking extreme")

  test "model picker recents then substring search":
    let root = freshDir()
    defer: removeDir(root)
    let cache = root / "models-dev.json"
    writeFile(cache, $(%*{
      "openrouter": {
        "models": {
          "deepseek/deepseek-v4-flash-0731": {"limit": {"context": 128000}},
          "anthropic/claude-sonnet-4": {"limit": {"context": 200000}}
        }
      },
      "anthropic": {
        "models": {
          "claude-sonnet-4-6": {"limit": {"context": 200000}}
        }
      }
    }))
    setModelsDevCachePath(cache)
    defer: setModelsDevCachePath("")
    let picker = ModelPicker(
      currentModel: "deepseek/deepseek-v4-flash-0731",
      defaultModel: "deepseek/deepseek-v4-flash-0731",
      currentProvider: "openrouter",
      keyedProviders: @["openrouter", "anthropic"])
    let recents = commandSuggestions("/model ", picker = picker)
    check recents == @["/model deepseek/deepseek-v4-flash-0731"]
    let short = commandSuggestions("/model d", picker = picker)
    check short == @["/model deepseek/deepseek-v4-flash-0731"]
    let hits = commandSuggestions("/model clau", picker = picker)
    check "/model anthropic/claude-sonnet-4" in hits
    check "/model claude-sonnet-4-6" in hits
    check commandSuggestionDescription("/model claude-sonnet-4-6") ==
      "anthropic  200k"
    check commandSuggestionDescription("/model deepseek/deepseek-v4-flash-0731") ==
      "openrouter  128k"

  test "catalog cache is stale when missing or old":
    let root = freshDir()
    defer: removeDir(root)
    let cache = root / "models-dev.json"
    setModelsDevCachePath(cache)
    defer: setModelsDevCachePath("")
    check modelsDevCacheStale()
    writeFile(cache, "{}")
    check not modelsDevCacheStale()
    setLastModificationTime(cache, fromUnix(1_000))
    check modelsDevCacheStale()

  test "/model switches provider when the catalog says so":
    let root = freshDir()
    defer: removeDir(root)
    let cache = root / "models-dev.json"
    writeFile(cache, $(%*{
      "openrouter": {"models": {"deepseek/x": {"limit": {"context": 1000}}}},
      "anthropic": {"models": {"claude-sonnet-4-6": {"limit": {"context": 200000}}}}
    }))
    setModelsDevCachePath(cache)
    defer: setModelsDevCachePath("")
    putEnv("OPENROUTER_API_KEY", "or-test")
    putEnv("ANTHROPIC_API_KEY", "an-test")
    defer:
      delEnv("OPENROUTER_API_KEY")
      delEnv("ANTHROPIC_API_KEY")
    var config = loadConfig(root, root / "config.json")
    config.sessionDir = root / "sessions"
    var agent = initAgent(config)
    check agent.config.provider == "openrouter"
    check agent.processInput("/model claude-sonnet-4-6")
    check agent.config.model == "claude-sonnet-4-6"
    check agent.config.provider == "anthropic"
    check agent.config.defaultModel == "deepseek/deepseek-v4-flash-0731"
    check agent.processInput("/model not-in-catalog")
    check agent.config.model == "not-in-catalog"
    check agent.config.provider == "anthropic"
    let again = loadConfig(root, root / "config.json")
    check again.model == "not-in-catalog"
    check again.provider == "anthropic"
    check again.defaultModel == "not-in-catalog"

  test "slash skill names expand and appear in suggestions":
    let root = freshDir()
    defer: removeDir(root)
    createDir(root / ".niminal" / "skills" / "review")
    writeFile(root / ".niminal" / "skills" / "review" / "SKILL.md",
      "---\nname: review\ndescription: Structured review.\n---\n\nBe thorough.\n")
    check "/review" in commandSuggestions("/re", root)
    check parseSlash("/review", root).kind == slSkill
    check parseSlash("/review", root).skillName == "review"
    check commandError("/review", root) == ""
    check commandError("/review the diff", root) == ""
    check "Unknown command" in commandError("/review", root / "empty")
    let expanded = expandSkillSlash(root, "/review src/foo.nim")
    check "Follow the \"review\" skill." in expanded
    check "Be thorough." in expanded
    check "src/foo.nim" in expanded
    check commandSuggestionDescription("/review", root) == "Structured review."

  test "menu rows use a filled selection on a charcoal panel":
    let selected = formatCommandMenuLine("/model", "show the current model",
      true, 8, 80)
    let idle = formatCommandMenuLine("/help", "show this help", false, 8, 80)
    check "\e[48;5;81m" in selected
    check "\e[30m" in selected
    check "/model" in selected
    check "show the current model" in selected
    check "\e[48;5;236m" in idle
    check "/help" in idle
    check "show this help" in idle
    let rows = formatCommandMenu(@["/model", "/models refresh"], 0, 2, 60)
    check rows.len == 2
    check "\e[48;5;81m" in rows[0]
    check "\e[48;5;236m" in rows[1]

  test "highlights malformed command input":
    let highlighted = highlightSlashCommand("/model refresh")
    check "\e[1;36m/model\e[0m" in highlighted
    check "\e[31m refresh\e[0m" in highlighted
    check parseSlash("/model refresh").kind == slError
    check "/models refresh" in commandSuggestions("/model refresh")

  test "resume suggestions list session ids with previews":
    let root = freshDir()
    defer: removeDir(root)
    var sess = initSession(root / "abc123.jsonl", "abc123")
    sess.addUserMessage("fix the parser")
    setLastModificationTime(root / "abc123.jsonl", fromUnix(1_000))
    check "/resume abc123" in commandSuggestions("/resume", root, root)
    check "/resume abc123" in commandSuggestions("/resume ", root, root)
    check "/resume abc123" in commandSuggestions("/resume ab", root, root)
    check commandSuggestions("/re", root, root).len > 0
    check "/resume abc123" notin commandSuggestions("/re", root, root)
    let desc = commandSuggestionDescription("/resume abc123", root, root)
    check "fix the parser" in desc
    let rows = formatCommandMenu(@["/resume abc123"], 0, 1, 80, root, root)
    check rows.len == 1
    check "fix the parser" in rows[0]
    check "/resume [ID]" in commandSuggestions("/resume")

  test "transcript replay paints a compact chat":
    var session = initSession()
    session.id = "s1"
    session.addUserMessage("fix the parser")
    session.addAssistantResponse(ProviderResponse(content: @[
      text("Looking."),
      toolUse("1", "read", %*{"path": "src/parser.nim"})
    ]))
    session.addToolResult(toolUse("1", "read", %*{"path": "src/parser.nim"}),
      "ok\nline2", false)
    session.addCompaction("summary", 0, 10)
    let text = transcriptLines(session).join("\n")
    check userRail in text
    check "fix the parser" in text
    check "Looking." in text
    check "● read" in text
    check "src/parser.nim" in text
    check "Context compacted" in text

  test "transcript replay shows thinking as a tool card":
    var session = initSession()
    session.addAssistantResponse(ProviderResponse(content: @[
      ContentBlock(kind: ckThinking, thinking: "I should inspect the parser.\nThen edit it."),
      text("Done.")
    ]))
    let text = transcriptLines(session).join("\n")
    check "● think" in text
    check "I should inspect the parser." in text
    check "Done." in text

  test "transcript replay keeps early events":
    var session = initSession()
    for i in 0 ..< 90:
      session.addUserMessage("turn " & $i)
    let text = transcriptLines(session).join("\n")
    check "earlier events omitted" notin text
    check "turn 0" in text
    check "turn 89" in text

  test "tool cards collapse long output and expand":
    let card = ToolCard(name: "bash", summary: "nimble test",
      output: "a\nb\nc\nd\ne")
    check isExpandable(card)
    proc bodies(c: ToolCard): seq[string] =
      for line in toolCardLines(c):
        result.add stripAnsi(line).replace("▌", "").strip
    let collapsed = bodies(card)
    check collapsed[0].startsWith("● bash")
    check "nimble test" in collapsed[0]
    check "▸" in collapsed[0]
    check "a" in collapsed
    check "b" in collapsed
    check "c" notin collapsed
    check "e" notin collapsed
    check collapsed.len == 3
    var open = card
    open.expanded = true
    let opened = bodies(open)
    check "c" in opened
    check "e" in opened
    check wrapLineCount("hello", 80) == 1
    check wrapLineCount("x".repeat(40), 10) > 1
    check "▾" in opened[0]
    check "collapse" notin opened.join(" ")

  test "collapsed tool errors preview the tail":
    let card = ToolCard(name: "bash", summary: "nimble test", isError: true,
      output: "exit_code: 1\nfoo\nbar\nbaz\nFAIL")
    proc bodies(c: ToolCard): seq[string] =
      for line in toolCardLines(c):
        result.add stripAnsi(line).replace("▌", "").strip
    let plains = bodies(card)
    check "FAIL" in plains
    check "baz" in plains
    check "exit_code: 1" notin plains
    check not isExpandable(ToolCard(name: "read", output: "ok\nline2"))

  test "edit hunk replaces ok body; write is all plus":
    let input = %*{"old_text": "before", "new_text": "after"}
    let plain = formatToolHunk("edit", input, false)
    check plain == @["- before", "+ after"]
    let colored = formatToolHunk("edit", input, true)
    check colored.len == 2
    check "\e[31m" in colored[0]
    check "- before" in colored[0]
    check "\e[32m" in colored[1]
    check "+ after" in colored[1]
    check formatToolHunk("write", %*{"content": "hello\nworld"}, false) ==
      @["+ hello", "+ world"]
    var session = initSession()
    let call = toolUse("1", "edit",
      %*{"path": "f.nim", "old_text": "before", "new_text": "after"})
    session.addAssistantResponse(ProviderResponse(content: @[call]))
    session.addToolResult(call, "OK — f.nim\nversion: abc", false)
    let replayed = stripAnsi(transcriptLines(session).join("\n"))
    check "- before" in replayed
    check "+ after" in replayed
    check "OK —" notin replayed
    check "+1" in replayed
    check "-1" in replayed

  test "collapsed preview skips tool headers":
    let bash = ToolCard(name: "bash", summary: "python3 hello_world.py",
      output: "exit_code: 0\nduration_ms: 112\n\nstdout:\nHello from niminal\n")
    let bashText = stripAnsi(toolCardLines(bash).join("\n"))
    check "Hello from niminal" in bashText
    check "exit_code" notin bashText
    check "duration_ms" notin bashText
    let read = ToolCard(name: "read", summary: "hello_world.py",
      output: "path: hello_world.py\nversion: abc\nlines: 1-1 of 1\n\n1 | print(\"hi\")\n")
    let readText = stripAnsi(toolCardLines(read).join("\n"))
    check "print" in readText
    check "version:" notin readText
    let hunk = formatToolHunk("edit",
      %*{"old_text": "a\nb\nc", "new_text": "a\nB\nc"}, true)
    let edit = ToolCard(name: "edit", summary: "f.nim", hunk: hunk,
      output: "OK — f.nim")
    let editText = stripAnsi(toolCardLines(edit).join("\n"))
    check "- a" in editText
    check "+ a" in editText
    check "- b" notin editText
    check "▸" in editText

  test "tool cards are separated by a blank line":
    var session = initSession()
    let a = toolUse("1", "read", %*{"path": "a.nim"})
    let b = toolUse("2", "read", %*{"path": "b.nim"})
    session.addAssistantResponse(ProviderResponse(content: @[a, b]))
    session.addToolResult(a, "path: a.nim\nversion: x\n\n1 | one", false)
    session.addToolResult(b, "path: b.nim\nversion: y\n\n1 | two", false)
    let lines = transcriptLines(session)
    var sawA, gapAfterA = false
    for i, line in lines:
      if "● read" in stripAnsi(line) and "a.nim" in stripAnsi(line):
        sawA = true
      elif sawA and not gapAfterA:
        if stripAnsi(line).len == 0:
          gapAfterA = true
    check gapAfterA

suite "project instructions and skills":
  test "loads AGENTS files from repository root to workspace":
    let root = freshDir()
    defer: removeDir(root)
    createDir(root / ".git")
    createDir(root / "backend")
    createDir(root / "frontend")
    writeFile(root / "AGENTS.md", "Use Nim.\n")
    writeFile(root / "backend" / "AGENTS.md", "Run backend tests.\n")
    writeFile(root / "frontend" / "AGENTS.md", "Use the frontend stack.\n")

    let isolated = root / "no-global.md"
    let paths = instructionPaths(root / "backend", isolated)
    let prompt = loadProjectInstructions(root / "backend", isolated)
    check paths.len == 2
    check prompt.find("Use Nim.") < prompt.find("Run backend tests.")
    check "Use the frontend stack." notin prompt

  test "global AGENTS.md precedes the repository chain":
    let root = freshDir()
    defer: removeDir(root)
    createDir(root / ".git")
    writeFile(root / "AGENTS.md", "Use Nim.\n")
    let global = root / "global-agents.md"
    writeFile(global, "Be terse.\n")
    let paths = instructionPaths(root, global)
    check paths.len == 2
    check paths[0] == global
    check paths[1] == expandFilename(root) / "AGENTS.md"
    let prompt = loadProjectInstructions(root, global)
    check prompt.find("Be terse.") < prompt.find("Use Nim.")
    check "path=\"global\"" in prompt

  test "discovers skill metadata and loads bodies lazily":
    let root = freshDir()
    defer: removeDir(root)
    createDir(root / ".niminal" / "skills" / "review")
    writeFile(root / ".niminal" / "skills" / "review" / "SKILL.md",
      "---\nname: review\ndescription: Review changes carefully.\n---\n\n" &
      "# Review\n\nKeep the secret detail.\n")

    let skills = discoverSkills(root)
    check skills.len >= 1
    var found = false
    for skill in skills:
      if skill.name == "review":
        found = true
        check skill.description == "Review changes carefully."
    check found
    let metadata = skillMetadataPrompt(root)
    check "review: Review changes carefully." in metadata
    check "Keep the secret detail." notin metadata
    let loaded = loadSkill(root, "review")
    check loaded.ok
    check "Keep the secret detail." in loaded.content
    let toolResult = invoke(makeSkillTool(root), %*{"name": "review"})
    check not toolResult.isError
    check "Keep the secret detail." in toolResult.output

  test ".niminal skills override .agent skills with the same name":
    let root = freshDir()
    defer: removeDir(root)
    createDir(root / ".agent" / "skills" / "review")
    createDir(root / ".niminal" / "skills" / "review")
    writeFile(root / ".agent" / "skills" / "review" / "SKILL.md",
      "---\nname: review\ndescription: From .agent.\n---\n")
    writeFile(root / ".niminal" / "skills" / "review" / "SKILL.md",
      "---\nname: review\ndescription: From .niminal.\n---\n")
    let skills = discoverSkills(root)
    var desc = ""
    for skill in skills:
      if skill.name == "review":
        desc = skill.description
    check desc == "From .niminal."

  test "agent request contains instructions and skill metadata":
    let root = freshDir()
    defer: removeDir(root)
    createDir(root / ".git")
    createDir(root / ".niminal" / "skills" / "testing")
    writeFile(root / "AGENTS.md", "Always run the focused test first.\n")
    writeFile(root / ".niminal" / "skills" / "testing" / "SKILL.md",
      "---\nname: testing\ndescription: Focused test workflow.\n---\n")
    var config = loadConfig(root)
    config.sessionDir = root / "sessions"
    config.contextWindow = 128_000
    let agent = initAgent(config)
    let request = agent.buildRequest()
    let system = request.system.join("\n")
    check "Always run the focused test first." in system
    check "testing: Focused test workflow." in system
    var hasSkillTool = false
    for definition in request.tools:
      if definition.name == "read_skill":
        hasSkillTool = true
    check hasSkillTool

  test "base system prompt is present without AGENTS.md":
    let root = freshDir()
    defer: removeDir(root)
    var config = loadConfig(root)
    config.sessionDir = root / "sessions"
    config.contextWindow = 128_000
    let request = initAgent(config).buildRequest()
    let system = request.system.join("\n")
    check "expected_version" in system
    check "Do not commit" in system
    check "read_skill" in system
    check "Project instructions" notin system

suite "models.dev catalog":
  test "lookup uses cached api.json without network":
    let root = freshDir()
    defer: removeDir(root)
    let cache = root / "models-dev.json"
    let fixture = %*{
      "openrouter": {
        "models": {
          "deepseek/deepseek-v4-flash-0731": {
            "id": "deepseek/deepseek-v4-flash-0731",
            "limit": {"context": 1_000_000, "output": 384_000}
          }
        }
      },
      "anthropic": {
        "models": {
          "claude-sonnet-4-6": {
            "id": "claude-sonnet-4-6",
            "limit": {"context": 200_000, "output": 64_000}
          }
        }
      }
    }
    writeFile(cache, $fixture)
    setModelsDevCachePath(cache)
    defer: setModelsDevCachePath("")
    check lookupContextWindow("openrouter", "deepseek/deepseek-v4-flash-0731") == 1_000_000
    check lookupContextWindow("anthropic", "claude-sonnet-4-6") == 200_000
    check lookupContextWindow("anthropic", "missing-model") == 0
    var config = loadConfig()
    config.provider = "openrouter"
    config.model = "deepseek/deepseek-v4-flash-0731"
    config.contextWindow = 0
    check config.effectiveContextWindow == 1_000_000
    config.contextWindow = 42_000
    check config.effectiveContextWindow == 42_000

  test "lookup missing provider does not crash":
    let root = freshDir()
    defer: removeDir(root)
    let cache = root / "models-dev.json"
    writeFile(cache, "{}")
    setModelsDevCachePath(cache)
    defer: setModelsDevCachePath("")
    check lookupContextWindow("openrouter", "any/model") == 0
    var config = loadConfig()
    config.provider = "openrouter"
    config.model = "any/model"
    config.contextWindow = 0
    check config.effectiveContextWindow == guessContextWindow(config.model)

suite "compaction":
  test "findCutIndex keeps a recent user-bound window":
    var session = initSession()
    # Older turns
    for i in 1..5:
      session.addUserMessage("old user " & $i & " " & "x".repeat(400))
      session.addAssistantResponse(ProviderResponse(
        content: @[text("old ass " & $i & " " & "y".repeat(400))]))
    # Recent turn
    session.addUserMessage("recent user " & "z".repeat(100))
    session.addAssistantResponse(ProviderResponse(
      content: @[text("recent ass " & "w".repeat(100))]))
    # keepRecent small so cut lands in the older region
    let cut = findCutIndex(session, keepRecentTokens = 80)
    check cut > 0
    check cut < session.events.len
    check session.events[cut].kind == sekUser

  test "messagesForModel uses summary + kept tail":
    var session = initSession()
    session.addUserMessage("ancient")
    session.addAssistantResponse(ProviderResponse(content: @[text("old reply")]))
    session.addUserMessage("recent")
    session.addAssistantResponse(ProviderResponse(content: @[text("new reply")]))
    # Keep from event index 2 ("recent")
    session.addCompaction("## Goal\nShip it", 2, 999)
    let msgs = session.messagesForModel
    check msgs.len >= 2
    check "<summary>" in msgs[0].content[0].text
    check "Ship it" in msgs[0].content[0].text
    check msgs[1].content[0].text == "recent"
    # Raw messages() still has everything
    check session.messages.len == 4

  test "shouldCompact respects reserve headroom":
    var session = initSession()
    session.addAssistantResponse(ProviderResponse(
      usage: Usage(inputTokens: 90_000),
      content: @[text("hi")]))
    check shouldCompact(session, 100_000, 16_384)
    check not shouldCompact(session, 100_000, 5_000)

  test "context estimate includes events after reported usage":
    var session = initSession()
    let call = toolUse("tool-1", "read", %*{"path": "large.txt"})
    session.addUserMessage("read the large file")
    session.addAssistantResponse(ProviderResponse(
      usage: Usage(inputTokens: 1_000),
      content: @[call]))
    session.addToolResult(call, "x".repeat(4_000), false)
    check estimatedContextTokens(session) > 1_000

  test "cut starts at the user turn before a large assistant response":
    var session = initSession()
    session.addUserMessage("old request")
    session.addAssistantResponse(ProviderResponse(content: @[text("old reply")]))
    session.addUserMessage("recent request")
    session.addAssistantResponse(ProviderResponse(
      content: @[text("z".repeat(1_000))]))
    check findCutIndex(session, keepRecentTokens = 10) == 2

  test "iterative summary prompt includes previous summary":
    let prompt = buildSummaryPrompt("prev bits", "user:\nhello\n", "keep DB work")
    check "<previous-summary>" in prompt
    check "prev bits" in prompt
    check "<compaction-instructions>" in prompt
    check "keep DB work" in prompt
    check "<conversation>" in prompt

  test "compaction event round-trips in JSONL":
    let root = freshDir()
    defer: removeDir(root)
    let path = root / "c.jsonl"
    var session = initSession(path, "c1")
    session.addUserMessage("a")
    session.addUserMessage("b")
    session.addCompaction("## Goal\nX", 1, 42)
    let reloaded = initSession(path, "c1")
    check reloaded.events.len == 3
    check reloaded.events[^1].kind == sekCompaction
    check reloaded.events[^1].summary == "## Goal\nX"
    check reloaded.events[^1].firstKeptIndex == 1
    check reloaded.events[^1].tokensBefore == 42

suite "json config":
  test "project overlays global and becomes the write target":
    let root = freshDir()
    defer: removeDir(root)
    let globalFile = root / "global.json"
    createDir(root / ".niminal")
    writeFile(globalFile, """{"default_model":"from-global","agent":{"max_tokens":1}}""")
    writeFile(root / ".niminal" / "config.json", """{"default_model":"from-project"}""")
    let config = loadConfig(root, "", globalFile)
    check config.model == "from-project"
    check config.maxTokens == 1
    check sameFile(config.writePath, root / ".niminal" / "config.json")

  test "persist patches model keys without dropping others":
    let root = freshDir()
    defer: removeDir(root)
    let path = root / "config.json"
    writeFile(path, """{"default_model":"old","agent":{"max_tokens":7}}""")
    var config = loadConfig(root, path)
    check config.maxTokens == 7
    config.model = "picked/id"
    config.provider = "anthropic"
    persistModel(config)
    let doc = parseJson(readFile(path))
    check doc["default_model"].getStr == "picked/id"
    check doc["default_provider"].getStr == "anthropic"
    check doc["agent"]["max_tokens"].getInt == 7

  test "persist patches thinking without dropping other agent keys":
    let root = freshDir()
    defer: removeDir(root)
    let path = root / "config.json"
    writeFile(path, """{"agent": {"max_tokens": 7, "thinking": "low"}}""")
    var config = loadConfig(root, path)
    config.sessionDir = root / "sessions"
    var agent = initAgent(config)
    check agent.setThinking("high") == "high"
    let doc = parseJson(readFile(path))
    check doc["agent"]["thinking"].getStr == "high"
    check doc["agent"]["max_tokens"].getInt == 7
    let again = loadConfig(root, path)
    check again.thinking == "high"

  test "persist creates global when no project config exists":
    let root = freshDir()
    defer: removeDir(root)
    let globalFile = root / "global.json"
    var config = loadConfig(root, "", globalFile)
    check config.writePath == globalFile
    config.model = "picked/id"
    config.provider = "openrouter"
    persistModel(config)
    check fileExists(globalFile)
    let doc = parseJson(readFile(globalFile))
    check doc["default_model"].getStr == "picked/id"
    check doc["default_provider"].getStr == "openrouter"

suite "thinking / reasoning options":
  test "openrouter options carry effort":
    let root = freshDir()
    defer: removeDir(root)
    writeFile(root / "models-dev.json", "{}")
    setModelsDevCachePath(root / "models-dev.json")
    defer: setModelsDevCachePath("")
    var config = loadConfig()
    config.provider = "openrouter"
    config.thinking = "high"
    let opts = providerOptions(config)
    check opts["reasoning"]["effort"].getStr == "high"

  test "anthropic options map to budget tokens":
    let root = freshDir()
    defer: removeDir(root)
    writeFile(root / "models-dev.json", "{}")
    setModelsDevCachePath(root / "models-dev.json")
    defer: setModelsDevCachePath("")
    var config = loadConfig()
    config.provider = "anthropic"
    config.thinking = "medium"
    let opts = providerOptions(config)
    check opts["thinking"]["type"].getStr == "enabled"
    check opts["thinking"]["budget_tokens"].getInt == 8000

  test "none omits anthropic thinking block":
    let root = freshDir()
    defer: removeDir(root)
    writeFile(root / "models-dev.json", "{}")
    setModelsDevCachePath(root / "models-dev.json")
    defer: setModelsDevCachePath("")
    var config = loadConfig()
    config.provider = "anthropic"
    config.thinking = "none"
    let opts = providerOptions(config)
    check "thinking" notin opts

  test "agent buildRequest includes reasoning":
    let root = freshDir()
    defer: removeDir(root)
    writeFile(root / "models-dev.json", "{}")
    setModelsDevCachePath(root / "models-dev.json")
    defer: setModelsDevCachePath("")
    writeFile(root / "config.json", """{"agent": {"thinking": "low"}}""")
    var config = loadConfig(root, root / "config.json")
    config.sessionDir = root / "sessions"
    config.provider = "openrouter"
    var agent = initAgent(config)
    let req = agent.buildRequest()
    check req.options["reasoning"]["effort"].getStr == "low"
    check agent.setThinking("high") == "high"
    check agent.buildRequest().options["reasoning"]["effort"].getStr == "high"

  test "snap effort to nearest catalog rung":
    check snapToEfforts("medium", ["high", "xhigh"]) == "high"
    check snapToEfforts("max", ["high", "xhigh"]) == "xhigh"
    check snapToEfforts("high", ["high", "xhigh"]) == "high"
    check snapToEfforts("none", ["high", "xhigh"]) == ""
    check snapToEfforts("low", ["low", "high", "max"]) == "low"
    check snapToEfforts("medium", ["low", "high"]) == "high"

  test "catalog caps snap, toggle, and hide unsupported":
    let root = freshDir()
    defer: removeDir(root)
    writeFile(root / "models-dev.json", $(%*{
      "openrouter": {
        "models": {
          "flash": {
            "reasoning": true,
            "reasoning_options": [{"type": "effort", "values": ["high", "xhigh"]}],
            "limit": {"context": 1000}
          },
          "toggle-only": {
            "reasoning": true,
            "reasoning_options": [{"type": "toggle"}],
            "limit": {"context": 1000}
          },
          "dumb": {"limit": {"context": 1000}}
        }
      }
    }))
    setModelsDevCachePath(root / "models-dev.json")
    defer: setModelsDevCachePath("")
    var config = loadConfig()
    config.provider = "openrouter"
    config.model = "flash"
    config.thinking = "medium"
    check providerOptions(config)["reasoning"]["effort"].getStr == "high"
    check thinkingStatus(config) == "high"
    check thinkingChoices("openrouter", "flash") == @["none", "high", "xhigh"]
    config.thinking = "none"
    check "reasoning" notin providerOptions(config)
    check thinkingStatus(config) == "off"
    config.model = "toggle-only"
    config.thinking = "medium"
    check providerOptions(config)["reasoning"]["enabled"].getBool
    check thinkingStatus(config) == "on"
    check thinkingChoices("openrouter", "toggle-only") == @["none", "high"]
    config.thinking = "none"
    check "reasoning" notin providerOptions(config)
    config.model = "dumb"
    config.thinking = "high"
    check providerOptions(config).len == 0
    check thinkingStatus(config) == ""
    check thinkingChoices("openrouter", "dumb").len == 0
    var agent = Agent(config: config, thinking: "high")
    check "think:" notin agent.statusFooter

suite "markdown rendering":
  test "plain mode strips punctuation":
    let rendered = renderMarkdown("## Heading\n\n**bold** and *italic* and `code`", false)
    check "Heading" in rendered
    check "bold" in rendered
    check "italic" in rendered
    check "code" in rendered
    check "\x1b" notin rendered

  test "colored mode adds ANSI codes":
    let rendered = renderMarkdown("**bold**", true)
    check "\x1b[1;93m" in rendered
    check "bold" in rendered

  test "bold italic combined":
    let plain = renderMarkdown("***both***", false)
    check "both" in plain
    check "*" notin plain
    let colored = renderMarkdown("***both***", true)
    check "\x1b[1;3;93m" in colored
    check "both" in colored

  test "fenced code blocks preserve content":
    let source = "```nim\nlet x = 42\n```"
    let plain = renderMarkdown(source, false)
    check "let x = 42" in plain
    let colored = renderMarkdown(source, true)
    check "let x = 42" in colored
    check "\x1b[2m" in colored

  test "links render as text with url":
    let plain = renderMarkdown("[docs](https://example.com)", false)
    check "docs" in plain
    check "https://example.com" in plain

  test "lists get bullet markers":
    let plain = renderMarkdown("- one\n- two", false)
    check "• one" in plain
    check "• two" in plain

  test "tables render with box characters":
    let source = "| Name | Value |\n|------|-------|\n| foo  | 1     |\n| bar  | 2     |"
    let plain = renderMarkdown(source, false)
    check "Name" in plain
    check "Value" in plain
    check "foo" in plain
    check "bar" in plain
    check "┌" in plain
    check "┬" in plain
    check "┼" in plain
    check "┴" in plain
    check "│" in plain

  test "colored tables have header emphasis":
    let source = "| H1 | H2 |\n|----|----|\n| a  | b  |"
    let colored = renderMarkdown(source, true)
    check "\x1b[1;93m" in colored

suite "ansi wrap":
  test "word-wraps long prose at spaces":
    let text = "Here is a deliberately long paragraph to see how wrapping behaves while text streams in."
    let lines = wrapAnsi(text, 40)
    check lines.len >= 2
    for line in lines:
      check line.replace("\x1b[0m", "").len <= 40
    check "deliberately" in lines.join(" ")
    check "streams" in lines.join(" ")

  test "preserves short lines":
    check wrapAnsi("hello", 40) == @["hello"]

  test "hard-breaks overlong tokens":
    let lines = wrapAnsi("abcdefghijklmnopqrstuvwxyz", 10)
    check lines.len >= 3
    check lines[0].startsWith("abcdefghij")

  test "box gutter repeats on wrapped rows":
    let line = "\e[35m│\e[0m " & "word ".repeat(20).strip
    let (prefix, body, gutter) = peelBoxGutter(line)
    check gutter == 2
    check prefix.endsWith(" ")
    check "│" in prefix
    let inner = 20
    var visual: seq[string] = @[]
    for part in wrapAnsi(body, inner):
      visual.add prefix & part
    check visual.len >= 2
    for row in visual:
      check row.contains("│")

  test "user card rail peels and repeats":
    let line = "\e[36m▌\e[0m\e[48;5;236m " & "word ".repeat(20).strip
    let (prefix, body, gutter) = peelBoxGutter(line)
    check gutter == 2
    check "▌" in prefix
    check body.startsWith("word")
    let rows = wrapAnsi(body, 20)
    check rows.len >= 2
    for part in rows:
      check "▌" in prefix & part

  test "plain stream wrap only rewrites the unfinished line":
    var cache: seq[string]
    var lineStart, tailRows: int
    growPlainWrap(cache, "hello", lineStart, tailRows, 80)
    check cache == @["hello"]
    check tailRows == 1
    check lineStart == 0
    growPlainWrap(cache, "hello world", lineStart, tailRows, 80)
    check cache == @["hello world"]
    growPlainWrap(cache, "hello world\nnext", lineStart, tailRows, 80)
    check cache[0] == "hello world"
    check cache[^1] == "next"
    check lineStart == "hello world\n".len
    let frozen = cache.len
    growPlainWrap(cache, "hello world\nnext line", lineStart, tailRows, 80)
    check cache.len == frozen
    check cache[0] == "hello world"
    check cache[^1] == "next line"

suite "composer and cost":
  test "paste normalizes CR LF to LF":
    check normalizePasteText("a\r\nb\rc") == "a\nb\nc"
    check normalizePasteText("plain") == "plain"

  test "Ctrl/Cmd+V CSI sequences count as paste":
    check isModifiedPaste("27;5;118~")
    check isModifiedPaste("27;9;118~")
    check isModifiedPaste("27;6;86~")
    check isModifiedPaste("118;5u")
    check isModifiedPaste("118;9u")
    check not isModifiedPaste("27;2;13~")
    check not isModifiedPaste("27;5;99~")
    check not isModifiedPaste("200~")

  test "utf-8 insert and delete stay on rune boundaries":
    var (s, cur) = insertAt("", 0, "é")
    check s == "é"
    check cur == s.len
    (s, cur) = insertAt(s, cur, "x")
    check s == "éx"
    (s, cur) = deleteBefore(s, cur)
    check s == "é"
    check cur == s.len
    (s, cur) = deleteBefore(s, cur)
    check s == ""
    check cur == 0
    (s, cur) = insertAt("ab", 1, "ü")
    check s == "aüb"
    (s, cur) = deleteAfter(s, 1)
    check s == "ab"

  test "composer wraps long lines and maps the cursor":
    check wrapRunes("abcdefghij", 4) == @["abcd", "efgh", "ij"]
    let view = composerView("abcdefghij", 5, 4)
    check view.lines == @["abcd", "efgh", "ij"]
    check view.row == 1
    check view.col == 1
    check visualToCursor("abcdefghij", 4, 1, 1) == 5
    let multi = composerView("short\n" & "x".repeat(10), 6, 4)
    check multi.lines.len >= 3
    check multi.row >= 1

  test "history jsonl round-trips and caps":
    let root = freshDir()
    defer: removeDir(root)
    let path = root / "history"
    saveComposerHistory(@["one", "two"], path)
    check loadComposerHistory(path) == @["one", "two"]
    var many: seq[string]
    for i in 0 ..< 520:
      many.add $i
    saveComposerHistory(many, path)
    let loaded = loadComposerHistory(path)
    check loaded.len == 500
    check loaded[0] == "20"
    check loaded[^1] == "519"

  test "usage cost uses catalog prices":
    let root = freshDir()
    defer: removeDir(root)
    let cache = root / "models-dev.json"
    writeFile(cache, $(%*{
      "openrouter": {
        "models": {
          "priced/model": {
            "limit": {"context": 1000},
            "cost": {"input": 1.0, "output": 2.0, "cache_read": 0.1}
          }
        }
      }
    }))
    setModelsDevCachePath(cache)
    defer: setModelsDevCachePath("")
    let usage = Usage(inputTokens: 1_000_000, outputTokens: 1_000_000)
    check estimateUsageCost("openrouter", "priced/model", usage) == 3.0
    check formatUsageCost("openrouter", "priced/model", usage) == "$3.00"
    let cached = Usage(inputTokens: 1_000_000, outputTokens: 0,
      cacheReadTokens: 500_000, cacheReported: true)
    # uncached 500k * $1 + cache 500k * $0.1 = $0.55
    check abs(estimateUsageCost("openrouter", "priced/model", cached) - 0.55) < 0.0001
    check formatUsageCost("openrouter", "missing", usage).len == 0

suite "file mentions":
  test "mentionAt finds @path and ignores emails":
    check mentionAt("user@host", 5).active == false
    check mentionAt("see @src/foo.nim", "see @src/foo.nim".len).query == "src/foo.nim"
    check mentionAt("@", 1).active
    check mentionAt("@", 1).query.len == 0
    check mentionAt("look at @src", 12).query == "src"
    check mentionAt("nope", 2).active == false

  test "applyMention splices the token":
    let ins = applyMention("fix @src", 8, "@src/agent.nim")
    check ins.text == "fix @src/agent.nim "
    check ins.cursor == ins.text.len

  test "suggestions and attach stay inside the workspace":
    let root = freshDir()
    defer: removeDir(root)
    createDir(root / "src")
    writeFile(root / "src" / "agent.nim", "proc foo = discard\n")
    writeFile(root / "README.md", "hello\n")
    writeFile(root / "src" / "bin.dat", "a\0b")
    let hits = commandSuggestions("see @ag", root, cursor = 7)
    check "@src/agent.nim" in hits
    check commandSuggestionDescription("@src/agent.nim") == "file"
    let expanded = expandMentions(root, "look at @src/agent.nim")
    check "look at @src/agent.nim" in expanded
    check "<file path=\"src/agent.nim\">" in expanded
    check "proc foo" in expanded
    check expandMentions(root, "look at @missing.nim") == "look at @missing.nim"
    check "<file" notin expandMentions(root, "see @src/bin.dat")
    check expandMentions(root, "user@host") == "user@host"

  test "workspace file list is cached until invalidate":
    let root = freshDir()
    defer: removeDir(root)
    invalidateWorkspaceFileList()
    writeFile(root / "a.txt", "a")
    check "a.txt" in listWorkspaceFiles(root)
    writeFile(root / "b.txt", "b")
    check "b.txt" notin listWorkspaceFiles(root)
    invalidateWorkspaceFileList()
    check "b.txt" in listWorkspaceFiles(root)

suite "images":
  const png = "\x89PNG\r\n\x1a\n" & "fake-png"
  const jpeg = "\xFF\xD8\xFF\xE0" & "fake-jpeg"
  const gif = "GIF89a" & "fake"
  const webp = "RIFF\x00\x00\x00\x00WEBP" & "fake"

  test "sniffs common image magic and rejects everything else":
    check sniffImageMime(png) == "image/png"
    check sniffImageMime(jpeg) == "image/jpeg"
    check sniffImageMime(gif) == "image/gif"
    check sniffImageMime(webp) == "image/webp"
    check sniffImageMime("not an image").len == 0
    check sniffImageMime("a\0b").len == 0
    let payload = imagePayload(png)
    check payload.ok
    check payload.mime == "image/png"
    check payload.data.len > 0
    var huge = png
    huge.add 'x'.repeat(MaxImageBytes)
    let over = imagePayload(huge)
    check over.mime == "image/png"
    check not over.ok
    check "too large" in over.err

  test "@mention and read attach images as blocks, not text":
    let root = freshDir()
    defer: removeDir(root)
    writeFile(root / "shot.jpg", jpeg)
    writeFile(root / "notes.txt", "hello\n")
    check expandMentions(root, "see @shot.jpg") == "see @shot.jpg"
    let blocks = expandUserContent(root, "see @shot.jpg and @notes.txt")
    check blocks.len == 2
    check blocks[0].kind == ckText
    check "see @shot.jpg" in blocks[0].text
    check "<file path=\"notes.txt\">" in blocks[0].text
    check "hello" in blocks[0].text
    check blocks[1].kind == ckImage
    check blocks[1].mimeType == "image/jpeg"
    check blocks[1].path == "shot.jpg"
    check blocks[1].data.len == 0
    let got = invoke(makeReadTool(initWorkspace(root)), %*{"path": "shot.jpg"})
    check not got.isError
    check "image/jpeg" in got.output
    check got.images.len == 1
    check got.images[0].mimeType == "image/jpeg"
    check got.images[0].path == "shot.jpg"
    check got.images[0].data.len == 0
    check got.output.count("\xFF") == 0

  test "session round-trips image blocks and tool images":
    let root = freshDir()
    defer: removeDir(root)
    let path = root / "img.jsonl"
    var sess = initSession(path, "img")
    sess.addUserMessage(@[text("look"), image("image/png", "QUJD")])
    sess.addToolResult(toolUse("1", "read", %*{"path": "shot.jpg"}),
      "path: shot.jpg\n", false, @[ImageContent(mimeType: "image/jpeg", data: "QUJD")])
    let loaded = initSession(path, "img")
    check loaded.events[0].message.content.len == 2
    check loaded.events[0].message.content[1].kind == ckImage
    check loaded.events[0].message.content[1].data == "QUJD"
    check loaded.events[1].toolImages.len == 1
    check loaded.events[1].toolImages[0].mimeType == "image/jpeg"
    let msgs = loaded.messagesForModel
    var found = false
    for m in msgs:
      for c in m.content:
        if c.kind == ckToolResult:
          found = true
          check c.images.len == 1
    check found

  test "dropImages and catalog: known text-only strips, unknown keeps":
    let root = freshDir()
    defer: removeDir(root)
    writeFile(root / "models-dev.json", $(%*{
      "openrouter": {"models": {
        "text-only": {
          "limit": {"context": 1000},
          "modalities": {"input": ["text"]}
        },
        "vision": {
          "limit": {"context": 1000},
          "modalities": {"input": ["text", "image"]}
        }
      }}
    }))
    setModelsDevCachePath(root / "models-dev.json")
    defer: setModelsDevCachePath("")
    check not lookupAcceptsImages("openrouter", "text-only")
    check lookupAcceptsImages("openrouter", "vision")
    check lookupAcceptsImages("openrouter", "mystery-model")
    let kept = @[userMessage(@[text("hi"), image("image/png", "QUJD")])]
    let dropped = dropImages(kept)
    check dropped[0].content.len == 2
    check dropped[0].content[1].kind == ckText
    check imageOmitted in dropped[0].content[1].text
    var config = loadConfig(root)
    config.sessionDir = root / "sessions"
    config.provider = "openrouter"
    config.model = "text-only"
    var agent = initAgent(config)
    agent.session.addUserMessage(@[text("hi"), image("image/png", "QUJD")])
    let req = agent.buildRequest()
    check req.messages[0].content[1].kind == ckText
    check imageOmitted in req.messages[0].content[1].text
    config.model = "vision"
    agent = initAgent(config)
    agent.session.addUserMessage(@[text("hi"), image("image/png", "QUJD")])
    let vis = agent.buildRequest()
    check vis.messages[0].content[1].kind == ckImage

  test "providers encode Pi image blocks; compaction omits bytes":
    check anthropicImageBlock("image/png", "QUJD")["source"]["data"].getStr == "QUJD"
    check "data:image/png;base64,QUJD" in $openAiImagePart("image/png", "QUJD")
    let body = buildBody(ProviderRequest(
      model: "vision",
      messages: @[userMessage(@[text("see"), image("image/png", "QUJD")])],
      maxTokens: 10), stream = false)
    let content = body["messages"][0]["content"]
    check content.kind == JArray
    check content.len == 2
    check content[1]["type"].getStr == "image_url"
    check "cache_control" in content[1]
    let sysBody = buildBody(ProviderRequest(
      model: "vision",
      system: @["stable prefix", "skills"],
      messages: @[userMessage("hi")],
      tools: @[ToolDefinition(name: "read", description: "d",
        inputSchema: %*{"type": "object"})],
      maxTokens: 10), stream = false)
    check sysBody["messages"][0]["role"].getStr == "system"
    let sysParts = sysBody["messages"][0]["content"]
    check sysParts.kind == JArray
    check sysParts.len == 2
    check "cache_control" in sysParts[1]
    check "cache_control" in sysBody["tools"][0]
    let toolBody = buildBody(ProviderRequest(
      model: "vision",
      messages: @[
        Message(role: roleAssistant, content: @[
          toolUse("1", "read", %*{"path": "a.png"})]),
        Message(role: roleUser, content: @[
          toolResult("1", "path: a.png", false,
            @[ImageContent(mimeType: "image/png", data: "QUJD")])])
      ], maxTokens: 10), stream = false)
    var sawTool = false
    var sawImageUser = false
    for m in toolBody["messages"]:
      if m["role"].getStr == "tool":
        sawTool = true
        check m["content"].getStr == "path: a.png"
      elif m["role"].getStr == "user" and m["content"].kind == JArray:
        for part in m["content"]:
          if part["type"].getStr == "image_url":
            sawImageUser = true
    check sawTool
    check sawImageUser
    var sess = initSession()
    sess.addUserMessage(@[text("see"), image("image/png", "QUJD" & "x".repeat(200))])
    let dumped = serializeRange(sess, 0, sess.events.len)
    check "[image/png]" in dumped
    check "QUJDx" notin dumped
    check imageTokenFallback == estimateEventTokens(sess.events[0]) -
      estimateTokens("see")

  test "clipboard ingest writes clips and path paste becomes @mention":
    let root = freshDir()
    defer: removeDir(root)
    writeFile(root / "shot.png", png)
    writeFile(root / "notes.txt", "hello\n")
    let ws = initWorkspace(root)
    let saved = saveWorkspaceImage(ws, png)
    check saved.ok
    check saved.mention.startsWith("@.niminal/clips/")
    check fileExists(root / saved.mention[1 .. ^1])
    let blocks = expandUserContent(root, "see " & saved.mention)
    check blocks.len == 2
    check blocks[1].kind == ckImage
    check ingestPastedPath(ws, root / "shot.png") == "@shot.png"
    check ingestPastedPath(ws, "\"" & root / "shot.png" & "\"") == "@shot.png"
    check ingestPastedPath(ws, "file://" & root / "shot.png") == "@shot.png"
    check ingestPastedPath(ws, root / "notes.txt").len == 0
    check ingestPastedPath(ws, "hello").len == 0
    check ingestPastedPath(ws, "shot.png\nand more").len == 0
    let outside = getTempDir() / ("niminal-out-" & $getCurrentProcessId() & ".jpg")
    writeFile(outside, jpeg)
    defer: removeFile(outside)
    let copied = ingestPastedPath(ws, outside)
    check copied.startsWith("@.niminal/clips/")
    check copied.endsWith(".jpg")

  test "PNG tile estimate and path-only session hydrate":
    proc be32(n: int): string =
      result = newString(4)
      result[0] = char((n shr 24) and 255)
      result[1] = char((n shr 16) and 255)
      result[2] = char((n shr 8) and 255)
      result[3] = char(n and 255)
    let header = "\x89PNG\r\n\x1a\n" & be32(13) & "IHDR" & be32(1568) & be32(1568)
    check imageDimensions(header) == (1568, 1568)
    let root = freshDir()
    defer: removeDir(root)
    writeFile(root / "tile.png", header)
    check imageTokenEstimate(ImageContent(mimeType: "image/png",
      path: root / "tile.png")) == 1600
    check imageTokenEstimate(ImageContent(mimeType: "image/png",
      path: "tile.png"), root) == 1600
    let path = root / "img.jsonl"
    var sess = initSession(path, "img2")
    sess.workspace = root
    sess.addUserMessage(@[text("look"), image("image/png", "", "tile.png")])
    let raw = readFile(path)
    check "tile.png" in raw
    check "\"data\"" notin raw
    let loaded = initSession(path, "img2")
    check loaded.events[0].message.content[1].path == "tile.png"
    check loaded.events[0].message.content[1].data.len == 0
    writeFile(root / "models-dev.json", $(%*{
      "openrouter": {"models": {
        "vision": {
          "limit": {"context": 1000},
          "modalities": {"input": ["text", "image"]}
        }
      }}
    }))
    setModelsDevCachePath(root / "models-dev.json")
    defer: setModelsDevCachePath("")
    var config = loadConfig(root)
    config.sessionDir = root / "sessions"
    config.provider = "openrouter"
    config.model = "vision"
    config.workspace = root
    var agent = initAgent(config)
    agent.session.addUserMessage(@[text("look"), image("image/png", "", "tile.png")])
    let req = agent.buildRequest()
    check req.messages[0].content[1].kind == ckImage
    check req.messages[0].content[1].data.len > 0
    check req.messages[0].content[1].path == "tile.png"

