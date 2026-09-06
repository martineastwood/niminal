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
import ../src/ui/theme
import ../src/models_dev
import ../src/compaction
import ../src/instructions
import ../src/skills
import ../src/commands
import nimgent
import nimgent/[anthropic, openrouter]
import ../src/tools/[tool, read_tool, edit_tool, write_tool, bash_tool, search_tool]
import ../src/extensions
import ../src/hooks
import ../src/main

proc freshDir(): string =
  result = getTempDir() / ("niminal-test-" & $getCurrentProcessId() & "-" &
    $int(epochTime() * 1_000_000))
  createDir(result)

proc invoke(pair: (ToolDefinition, ToolProc), input: JsonNode): ToolResult =
  pair[1](input)

proc writeExt(root, folder, name, runBody: string, timeout = 30) =
  let dir = root / folder / "tools" / name
  createDir(dir)
  var manifest = %*{
    "name": name,
    "description": "Test tool " & name,
    "command": ["./run"],
    "input_schema": {"type": "object", "properties": {}}
  }
  if timeout != 30:
    manifest["timeout_seconds"] = %timeout
  writeFile(dir / "tool.json", $manifest)
  writeFile(dir / "run", "#!/bin/sh\n" & runBody & "\n")
  inclFilePermissions(dir / "run", {fpUserExec, fpGroupExec, fpOthersExec})

proc writeHook(root, folder, name, event, runBody: string,
               tools: seq[string] = @[], timeout = 30) =
  let dir = root / folder / "hooks" / name
  createDir(dir)
  var manifest = %*{
    "name": name,
    "event": event,
    "command": ["./run"]
  }
  if tools.len > 0:
    manifest["tools"] = %tools
  if timeout != 30:
    manifest["timeout_seconds"] = %timeout
  writeFile(dir / "hook.json", $manifest)
  writeFile(dir / "run", "#!/bin/sh\n" & runBody & "\n")
  inclFilePermissions(dir / "run", {fpUserExec, fpGroupExec, fpOthersExec})

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

  test "edit applies several unique replacements or none":
    let root = freshDir()
    defer: removeDir(root)
    let path = root / "sample.txt"
    writeFile(path, "aaa\nbbb\nccc\n")
    let edit = makeEditTool(initWorkspace(root))
    let ok = invoke(edit, %*{
      "path": "sample.txt",
      "replacements": [
        {"old_text": "aaa", "new_text": "AAA"},
        {"old_text": "ccc", "new_text": "CCC"}
      ]
    })
    check not ok.isError
    check "replacements: 2" in ok.output
    check readFile(path) == "AAA\nbbb\nCCC\n"
    writeFile(path, "same\nsame\nkeep\n")
    let bad = invoke(edit, %*{
      "path": "sample.txt",
      "replacements": [
        {"old_text": "keep", "new_text": "kept"},
        {"old_text": "same", "new_text": "x"}
      ]
    })
    check bad.isError
    check readFile(path) == "same\nsame\nkeep\n"

  test "grep and glob find workspace files":
    let root = freshDir()
    defer: removeDir(root)
    createDir(root / "src")
    writeFile(root / "src" / "a.nim", "proc hello =\n  discard\n")
    writeFile(root / "src" / "b.txt", "hello world\n")
    writeFile(root / "readme.md", "hello docs\n")
    check globMatch("src/a.nim", "**/*.nim")
    check globMatch("a.nim", "**/*.nim")
    check globMatch("src/a.nim", "src/*")
    check not globMatch("src/a.nim", "*.nim")
    let grep = invoke(makeGrepTool(initWorkspace(root)),
      %*{"pattern": "hello", "glob": "**/*.nim"})
    check not grep.isError
    check "src/a.nim:1:" in grep.output
    check "b.txt" notin grep.output
    let grepRe = invoke(makeGrepTool(initWorkspace(root)),
      %*{"pattern": "proc\\s+hello"})
    check not grepRe.isError
    check "src/a.nim:1:" in grepRe.output
    let badRe = invoke(makeGrepTool(initWorkspace(root)), %*{"pattern": "("})
    check badRe.isError
    check "invalid pattern" in badRe.output
    let glob = invoke(makeGlobTool(initWorkspace(root)),
      %*{"pattern": "**/*.txt"})
    check not glob.isError
    check "src/b.txt" in glob.output
    check "a.nim" notin glob.output

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

  test "tool use parse_error round-trips":
    let root = freshDir()
    defer: removeDir(root)
    let path = root / "pe.jsonl"
    var original = initSession(path, "pe")
    original.addAssistantResponse(ProviderResponse(content: @[
      toolUseFromArgs("c1", "echo", "{nope")]))
    let recovered = initSession(path, "pe")
    check invalidToolCall(recovered.messages[0].content[0]).len > 0

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
    var orphan = initSession(root / "orphan.jsonl", "orphan")
    orphan.addUserMessage("no header")
    setLastModificationTime(root / "there.jsonl", fromUnix(3_000))
    setLastModificationTime(root / "here.jsonl", fromUnix(2_000))
    setLastModificationTime(root / "orphan.jsonl", fromUnix(1_000))
    let reloaded = initSession(root / "here.jsonl", "here")
    check reloaded.workspace == "/proj/here"
    check reloaded.events.len == 1
    let raw = readFile(root / "here.jsonl")
    check raw.startsWith("{\"type\":\"session\"")
    check listSessionIds(root, "/proj/here") == @["here"]
    check listSessionIds(root, "/proj/there") == @["there"]
    check listSessionIds(root) == @["there", "here", "orphan"]
    let infos = listSessions(root, "/proj/here")
    check infos.len == 1
    check infos[0].id == "here"
    check infos[0].workspace == "/proj/here"
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

  test "plugin dirs are global, then .agent, then .niminal":
    let root = freshDir()
    defer: removeDir(root)
    let ws = expandFilename(root)
    check pluginRoots(root, "hooks") == @[
      niminalConfigDir() / "hooks",
      ws / ".agent" / "hooks",
      ws / ".niminal" / "hooks"]
    createDir(root / ".agent" / "hooks" / "x")
    writeFile(root / ".agent" / "hooks" / "x" / "hook.json", "{}")
    createDir(root / ".niminal" / "hooks" / "y")
    writeFile(root / ".niminal" / "hooks" / "y" / "hook.json", "{}")
    check collectPluginDirs(root, "hooks", "hook.json") ==
      @[ws / ".agent" / "hooks" / "x", ws / ".niminal" / "hooks" / "y"]

  test "openai provider defaults and thinking map to reasoning.effort":
    let root = freshDir()
    defer: removeDir(root)
    writeFile(root / "models-dev.json", "{}")
    setModelsDevCachePath(root / "models-dev.json")
    defer: setModelsDevCachePath("")
    writeFile(root / "config.json", """{"default_provider":"openai"}""")
    var config = loadConfig(root, root / "config.json")
    check config.provider == "openai"
    check config.model == "gpt-5"
    check config.apiKeyEnv == "OPENAI_API_KEY"
    check config.endpoint == "https://api.openai.com/v1/responses"
    config.thinking = "high"
    check providerOptions(config)["reasoning"]["effort"].getStr == "high"
    config.thinking = "none"
    check "reasoning" notin providerOptions(config)

  test "hyper provider defaults and thinking map to reasoning.effort":
    let root = freshDir()
    defer: removeDir(root)
    writeFile(root / "models-dev.json", "{}")
    setModelsDevCachePath(root / "models-dev.json")
    defer: setModelsDevCachePath("")
    writeFile(root / "config.json", """{"default_provider":"hyper"}""")
    var config = loadConfig(root, root / "config.json")
    check config.provider == "hyper"
    check config.model == "deepseek-v4-flash"
    check config.apiKeyEnv == "HYPER_API_KEY"
    check config.endpoint == "https://hyper.charm.land/v1/chat/completions"
    config.thinking = "high"
    check providerOptions(config)["reasoning"]["effort"].getStr == "high"
    config.thinking = "none"
    check "reasoning" notin providerOptions(config)

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
    check "/provider [name]" in commandSuggestions("/pr")
    check "/thinking high" in commandSuggestions("/thinking ")
    check "/provider hyper" in commandSuggestions("/provider ")
    check "/provider hyper" in commandSuggestions("/provider hy")
    check parseSlash("/help").kind == slHelp
    check parseSlash("/provider").kind == slProvider
    check parseSlash("/provider").arg.len == 0
    check parseSlash("/provider hyper").kind == slProvider
    check parseSlash("/provider hyper").arg == "hyper"
    check "Unknown provider" in commandError("/provider nope")
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
    check parseSlash("/reload").kind == slReload
    check "takes no arguments" in commandError("/reload extra")
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
      currentProvider: "openrouter")
    let recents = commandSuggestions("/model ", picker = picker)
    check recents == @["/model deepseek/deepseek-v4-flash-0731"]
    let short = commandSuggestions("/model d", picker = picker)
    check short == @["/model deepseek/deepseek-v4-flash-0731"]
    let hits = commandSuggestions("/model clau", picker = picker)
    check "/model anthropic/claude-sonnet-4" in hits
    check "/model claude-sonnet-4-6" notin hits
    let other = ModelPicker(
      currentModel: "claude-sonnet-4-6",
      defaultModel: "claude-sonnet-4-6",
      currentProvider: "anthropic")
    let otherHits = commandSuggestions("/model clau", picker = other)
    check "/model claude-sonnet-4-6" in otherHits
    check "/model anthropic/claude-sonnet-4" notin otherHits
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

  test "/model stays on the current provider":
    let root = freshDir()
    defer: removeDir(root)
    let cache = root / "models-dev.json"
    writeFile(cache, $(%*{
      "openrouter": {"models": {"deepseek/x": {"limit": {"context": 1000}}}},
      "openai": {"models": {"gpt-5": {"limit": {"context": 1048576}}}},
      "anthropic": {"models": {"claude-sonnet-4-6": {"limit": {"context": 200000}}}}
    }))
    setModelsDevCachePath(cache)
    defer: setModelsDevCachePath("")
    putEnv("OPENROUTER_API_KEY", "or-test")
    putEnv("OPENAI_API_KEY", "oa-test")
    putEnv("ANTHROPIC_API_KEY", "an-test")
    defer:
      delEnv("OPENROUTER_API_KEY")
      delEnv("OPENAI_API_KEY")
      delEnv("ANTHROPIC_API_KEY")
    var config = loadConfig(root, root / "config.json")
    config.sessionDir = root / "sessions"
    var agent = initAgent(config)
    check agent.config.provider == "openrouter"
    check agent.processInput("/model gpt-5")
    check agent.config.model == "gpt-5"
    check agent.config.provider == "openrouter"
    check agent.processInput("/model claude-sonnet-4-6")
    check agent.config.model == "claude-sonnet-4-6"
    check agent.config.provider == "openrouter"
    check agent.processInput("/provider anthropic")
    check agent.config.provider == "anthropic"
    check agent.config.model == "claude-sonnet-4-6"
    check agent.processInput("/model not-in-catalog")
    check agent.config.model == "not-in-catalog"
    check agent.config.provider == "anthropic"
    let again = loadConfig(root, root / "config.json")
    check again.model == "not-in-catalog"
    check again.provider == "anthropic"

  test "/provider switches wired providers without a catalog entry":
    let root = freshDir()
    defer: removeDir(root)
    writeFile(root / "models-dev.json", "{}")
    setModelsDevCachePath(root / "models-dev.json")
    defer: setModelsDevCachePath("")
    writeFile(root / "config.json", """{"default_provider":"openrouter"}""")
    var config = loadConfig(root, root / "config.json")
    config.sessionDir = root / "sessions"
    var agent = initAgent(config)
    check agent.config.provider == "openrouter"
    check agent.processInput("/provider hyper")
    check agent.config.provider == "hyper"
    check agent.provider.name == "hyper"
    check agent.config.endpoint == "https://hyper.charm.land/v1/chat/completions"
    check agent.processInput("/model deepseek-v4-pro")
    check agent.config.model == "deepseek-v4-pro"
    check agent.config.provider == "hyper"
    let again = loadConfig(root, root / "config.json")
    check again.provider == "hyper"
    check again.model == "deepseek-v4-pro"

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
    let expanded = expandSkill(root, parseSlash("/review src/foo.nim", root))
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
    check Dark256.text in idle
    check "/help" in idle
    check "show this help" in idle
    let rows = formatCommandMenu(@["/model", "/models refresh"], 0, 2, 60)
    check rows.len == 2
    check "\e[48;5;81m" in rows[0]
    check "\e[48;5;236m" in rows[1]
    check suggestionStep(-1, -1, 3) == 0
    check suggestionStep(-1, 1, 3) == 0
    check suggestionStep(0, -1, 3) == 0
    check suggestionStep(0, 1, 3) == 1
    check suggestionStep(2, 1, 3) == 2

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
    sess.workspace = root
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
    check userRail() in text
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
    check formatToolHunk("edit", %*{"replacements": [
      {"old_text": "a", "new_text": "A"},
      {"old_text": "b", "new_text": "B"}
    ]}, false) == @["- a", "+ A", "- b", "+ B"]
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
    config.thinking = "high"
    var agent = Agent(config: config)
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

  test "stream paint coalesces unless newline or forced":
    check streamPaintDue(0, 0.01, "hi") == false
    check streamPaintDue(0, 0.04, "hi")
    check streamPaintDue(0, 0.01, "hi\n")
    check streamPaintDue(0, 0.0, "hi", force = true)

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
    check findMentions("see @a and\n@b and @a") == @["a", "b"]

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

  test "workspace file list sees files written after the first call":
    let root = freshDir()
    defer: removeDir(root)
    writeFile(root / "a.txt", "a")
    check "a.txt" in listWorkspaceFiles(root)
    writeFile(root / "b.txt", "b")
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

suite "external tools":
  proc findExt(tools: seq[ExtensionTool], name: string): ExtensionTool =
    for t in tools:
      if t.name == name:
        return t
    raise newException(ValueError, "extension not found: " & name)

  test "valid manifest registers; broken tool.json warns and is skipped":
    let root = freshDir()
    defer: removeDir(root)
    writeExt(root, ".niminal", "echo_ok", "cat >/dev/null\necho '{\"ok\":true}'")
    createDir(root / ".niminal" / "tools" / "broken")
    writeFile(root / ".niminal" / "tools" / "broken" / "tool.json", "{not json")
    var reg: ToolRegistry
    let warnings = reg.registerExtensions(root)
    var hasBroken = false
    for w in warnings:
      if "broken" in w:
        hasBroken = true
    check hasBroken
    var names: seq[string] = @[]
    for d in reg.definitions:
      names.add d.name
    check "echo_ok" in names
    check "broken" notin names

  test ".niminal tools override .agent tools with the same name":
    let root = freshDir()
    defer: removeDir(root)
    writeExt(root, ".agent", "shared", "cat >/dev/null\necho '{\"from\":\".agent\"}'")
    writeExt(root, ".niminal", "shared", "cat >/dev/null\necho '{\"from\":\".niminal\"}'")
    let discovered = discoverExtensions(root)
    let shared = findExt(discovered.tools, "shared")
    check ".niminal" in shared.dir
    let result = runExtension(shared, %*{}, root)
    check not result.isError
    check "\".niminal\"" in result.output

  test "builtin name is skipped with a warning":
    let root = freshDir()
    defer: removeDir(root)
    writeExt(root, ".niminal", "bash", "echo '{\"nope\":true}'")
    var reg: ToolRegistry
    let warnings = reg.registerExtensions(root)
    var collision = false
    for w in warnings:
      if "bash" in w and "built-in" in w:
        collision = true
    check collision
    for d in reg.definitions:
      check d.name != "bash"

  test "script extension succeeds":
    let root = freshDir()
    defer: removeDir(root)
    writeExt(root, ".niminal", "greet",
      "cat >/dev/null\necho '{\"hello\":\"world\"}'")
    let ext = findExt(discoverExtensions(root).tools, "greet")
    let result = runExtension(ext, %*{"x": 1}, root)
    check not result.isError
    check "\"hello\"" in result.output
    check "\"world\"" in result.output

  test "binary extension succeeds":
    let root = freshDir()
    defer: removeDir(root)
    let dir = root / ".niminal" / "tools" / "echo_json"
    createDir(dir)
    writeFile(dir / "tool.json", $(%*{
      "name": "echo_json",
      "description": "Echo a JSON object",
      "command": ["/bin/echo", "{\"bin\":true}"],
      "input_schema": {"type": "object", "properties": {}}
    }))
    let ext = findExt(discoverExtensions(root).tools, "echo_json")
    let result = runExtension(ext, %*{}, root)
    check not result.isError
    check "\"bin\"" in result.output

  test "nonzero exit is an error":
    let root = freshDir()
    defer: removeDir(root)
    writeExt(root, ".niminal", "fail",
      "echo '{\"error\":\"nope\"}'\nexit 1")
    let ext = findExt(discoverExtensions(root).tools, "fail")
    let result = runExtension(ext, %*{}, root)
    check result.isError
    check "\"error\"" in result.output

  test "timeout_seconds expires a sleeping tool":
    let root = freshDir()
    defer: removeDir(root)
    writeExt(root, ".niminal", "slow", "sleep 5\necho '{}'", timeout = 1)
    let ext = findExt(discoverExtensions(root).tools, "slow")
    let result = runExtension(ext, %*{}, root)
    check result.isError
    check "TIMEOUT" in result.output

  test "non-JSON stdout is an error and includes raw text":
    let root = freshDir()
    defer: removeDir(root)
    writeExt(root, ".niminal", "plain", "cat >/dev/null\necho not-json-at-all")
    let ext = findExt(discoverExtensions(root).tools, "plain")
    let result = runExtension(ext, %*{}, root)
    check result.isError
    check "not valid JSON" in result.output
    check "not-json-at-all" in result.output

suite "lifecycle hooks":
  proc findHook(hooks: seq[Hook], name: string): Hook =
    for h in hooks:
      if h.name == name:
        return h
    raise newException(ValueError, "hook not found: " & name)

  proc quietUi(): TurnSink =
    TurnSink(
      emit: proc(level: MsgLevel, text: string) = discard,
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

  test "invalid hook.json warns and is skipped":
    let root = freshDir()
    defer: removeDir(root)
    writeHook(root, ".niminal", "ok", "session_start", "echo '{}'")
    createDir(root / ".niminal" / "hooks" / "broken")
    writeFile(root / ".niminal" / "hooks" / "broken" / "hook.json", "{nope")
    let discovered = discoverHooks(root)
    var hasBroken = false
    for w in discovered.warnings:
      if "broken" in w:
        hasBroken = true
    check hasBroken
    var names: seq[string] = @[]
    for h in discovered.hooks:
      names.add h.name
    check "ok" in names
    check "broken" notin names

  test ".niminal hooks override .agent hooks with the same name":
    let root = freshDir()
    defer: removeDir(root)
    writeHook(root, ".agent", "shared", "session_start",
      "echo '{\"from\":\".agent\"}'")
    writeHook(root, ".niminal", "shared", "session_start",
      "echo '{\"from\":\".niminal\"}'")
    let shared = findHook(discoverHooks(root).hooks, "shared")
    check ".niminal" in shared.dir

  test "pre_tool_call allow false skips the tool":
    let root = freshDir()
    defer: removeDir(root)
    writeHook(root, ".niminal", "block", "pre_tool_call",
      "cat >/dev/null\necho '{\"allow\":false,\"reason\":\"nope\"}'")
    writeFile(root / "marker.txt", "keep")
    var config = loadConfig(root, root / "config.json")
    config.sessionDir = root / "sessions"
    config.workspace = root
    config.compactionEnabled = false
    config.contextWindow = 1_000_000
    let hooks = discoverHooks(root).hooks
    var ran = false
    var reg: ToolRegistry
    reg.register(
      ToolDefinition(name: "touch", description: "t",
        inputSchema: %*{"type": "object"}),
      proc(input: JsonNode): ToolResult =
        ran = true
        writeFile(root / "ran.txt", "yes")
        ToolResult(output: "ran", isError: false))
    let provider = TestProvider(
      name: "test",
      responses: @[
        ProviderResponse(content: @[
          toolUse("1", "touch", %*{})]),
        ProviderResponse(content: @[text("done")])
      ])
    var agent = Agent(config: config, provider: provider,
      session: initSession(), tools: reg, hooks: hooks)
    agent.session.workspace = root
    agent.session.addUserMessage("go")
    agent.runTurn(quietUi())
    check not ran
    check not fileExists(root / "ran.txt")
    check agent.session.events.len >= 2
    var sawDeny = false
    for e in agent.session.events:
      if e.kind == sekToolResult and e.toolError and "nope" in e.toolOutput:
        sawDeny = true
    check sawDeny

  test "broken pre hook fails open and the tool still runs":
    let root = freshDir()
    defer: removeDir(root)
    writeHook(root, ".niminal", "broken", "pre_tool_call",
      "cat >/dev/null\necho not-json")
    var config = loadConfig(root, root / "config.json")
    config.sessionDir = root / "sessions"
    config.workspace = root
    config.compactionEnabled = false
    config.contextWindow = 1_000_000
    let hooks = discoverHooks(root).hooks
    var ran = false
    var reg: ToolRegistry
    reg.register(
      ToolDefinition(name: "touch", description: "t",
        inputSchema: %*{"type": "object"}),
      proc(input: JsonNode): ToolResult =
        ran = true
        ToolResult(output: "ran", isError: false))
    let provider = TestProvider(
      name: "test",
      responses: @[
        ProviderResponse(content: @[toolUse("1", "touch", %*{})]),
        ProviderResponse(content: @[text("done")])
      ])
    var agent = Agent(config: config, provider: provider,
      session: initSession(), tools: reg, hooks: hooks)
    agent.session.workspace = root
    agent.session.addUserMessage("go")
    var warns: seq[string] = @[]
    var ui = quietUi()
    ui.emit = proc(level: MsgLevel, text: string) =
      if level == mlWarn: warns.add text
    agent.runTurn(ui)
    check ran
    check warns.len >= 1
    check "broken" in warns[0]

  test "any matching pre hook deny blocks":
    let root = freshDir()
    defer: removeDir(root)
    writeHook(root, ".niminal", "allow-a", "pre_tool_call",
      "cat >/dev/null\necho '{\"allow\":true}'")
    writeHook(root, ".niminal", "deny-b", "pre_tool_call",
      "cat >/dev/null\necho '{\"allow\":false,\"reason\":\"second\"}'")
    let outcome = runHooks(discoverHooks(root).hooks, hePreToolCall,
      preToolPayload("bash", %*{"command": "ls"}), root, "bash")
    check not outcome.allowed
    check "second" in outcome.reason

  test "tools filter skips non-matching tools":
    let root = freshDir()
    defer: removeDir(root)
    writeHook(root, ".niminal", "bash-only", "pre_tool_call",
      "cat >/dev/null\necho '{\"allow\":false,\"reason\":\"bash-blocked\"}'",
      tools = @["bash"])
    let hooks = discoverHooks(root).hooks
    let forRead = runHooks(hooks, hePreToolCall,
      preToolPayload("read", %*{"path": "x"}), root, "read")
    check forRead.allowed
    let forBash = runHooks(hooks, hePreToolCall,
      preToolPayload("bash", %*{"command": "x"}), root, "bash")
    check not forBash.allowed
    check "bash-blocked" in forBash.reason

  test "post_tool_call receives output and is_error":
    let root = freshDir()
    defer: removeDir(root)
    writeHook(root, ".niminal", "capture", "post_tool_call",
      "cat > post-input.json\necho '{}'")
    let hooks = discoverHooks(root).hooks
    let outcome = runHooks(hooks, hePostToolCall,
      postToolPayload("bash", %*{"command": "echo hi"}, "exit_code: 0", false),
      root, "bash")
    check outcome.allowed
    check fileExists(root / "post-input.json")
    let doc = parseJson(readFile(root / "post-input.json"))
    check doc["tool"].getStr == "bash"
    check doc["output"].getStr == "exit_code: 0"
    check doc["is_error"].getBool == false

  test "/new fires session_end then session_start":
    let root = freshDir()
    defer: removeDir(root)
    writeHook(root, ".niminal", "log-end", "session_end",
      "echo end >> hook-log.txt\necho '{}'")
    writeHook(root, ".niminal", "log-start", "session_start",
      "echo start >> hook-log.txt\necho '{}'")
    var config = loadConfig(root, root / "config.json")
    config.sessionDir = root / "sessions"
    config.workspace = root
    var agent = initAgent(config)
    check agent.hooks.len >= 2
    if fileExists(root / "hook-log.txt"):
      removeFile(root / "hook-log.txt")
    check agent.processInput("/new", quietUi())
    check fileExists(root / "hook-log.txt")
    let log = readFile(root / "hook-log.txt")
    check log == "end\nstart\n"

  test "/new rescans tools and hooks from disk":
    let root = freshDir()
    defer: removeDir(root)
    var config = loadConfig(root, root / "config.json")
    config.sessionDir = root / "sessions"
    config.workspace = root
    var agent = initAgent(config)
    var names: seq[string] = @[]
    for d in agent.tools.definitions:
      names.add d.name
    check "late_tool" notin names
    check agent.hooks.len == 0
    writeExt(root, ".niminal", "late_tool",
      "cat >/dev/null\necho '{\"ok\":true}'")
    writeHook(root, ".niminal", "late_hook", "session_start",
      "echo late >> late-log.txt\necho '{}'")
    check agent.processInput("/new", quietUi())
    names = @[]
    for d in agent.tools.definitions:
      names.add d.name
    check "late_tool" in names
    var foundHook = false
    for h in agent.hooks:
      if h.name == "late_hook":
        foundHook = true
    check foundHook
    check fileExists(root / "late-log.txt")
    check "late" in readFile(root / "late-log.txt")

  test "/reload rescans tools and hooks without a new session":
    let root = freshDir()
    defer: removeDir(root)
    var config = loadConfig(root, root / "config.json")
    config.sessionDir = root / "sessions"
    config.workspace = root
    var agent = initAgent(config)
    let sessionId = agent.session.id
    var names: seq[string] = @[]
    for d in agent.tools.definitions:
      names.add d.name
    check "late_tool" notin names
    check agent.hooks.len == 0
    writeExt(root, ".niminal", "late_tool",
      "cat >/dev/null\necho '{\"ok\":true}'")
    writeHook(root, ".niminal", "late_hook", "session_start",
      "echo late >> late-log.txt\necho '{}'")
    check agent.processInput("/reload", quietUi())
    check agent.session.id == sessionId
    names = @[]
    for d in agent.tools.definitions:
      names.add d.name
    check "late_tool" in names
    var foundHook = false
    for h in agent.hooks:
      if h.name == "late_hook":
        foundHook = true
    check foundHook
    check not fileExists(root / "late-log.txt")

  test "pre_tool_call can rewrite arguments":
    let root = freshDir()
    defer: removeDir(root)
    writeHook(root, ".niminal", "rewrite", "pre_tool_call",
      """cat >/dev/null
echo '{"arguments":{"command":"echo rewritten"}}'
""")
    let outcome = runHooks(discoverHooks(root).hooks, hePreToolCall,
      preToolPayload("bash", %*{"command": "echo original"}), root, "bash")
    check outcome.allowed
    check not outcome.arguments.isNil
    check outcome.arguments["command"].getStr == "echo rewritten"

  test "post_tool_call can rewrite output and is_error":
    let root = freshDir()
    defer: removeDir(root)
    writeHook(root, ".niminal", "redact", "post_tool_call",
      """cat >/dev/null
echo '{"output":"[REDACTED]","is_error":false}'
""")
    let outcome = runHooks(discoverHooks(root).hooks, hePostToolCall,
      postToolPayload("read", %*{"path": ".env"}, "SECRET=1", false),
      root, "read")
    check outcome.hasOutput
    check outcome.output == "[REDACTED]"
    check outcome.hasIsError
    check not outcome.isError

  test "pre_compact can cancel or inject instruction":
    let root = freshDir()
    defer: removeDir(root)
    writeHook(root, ".niminal", "guide", "pre_compact",
      """cat >/dev/null
echo '{"instruction":"keep the migration work"}'
""")
    let guided = runHooks(discoverHooks(root).hooks, hePreCompact,
      preCompactPayload("s1", root, "", 1000), root)
    check guided.allowed
    check guided.instruction == "keep the migration work"
    writeHook(root, ".niminal", "block-compact", "pre_compact",
      """cat >/dev/null
echo '{"allow":false,"reason":"not now"}'
""")
    var config = loadConfig(root, root / "config.json")
    config.sessionDir = root / "sessions"
    config.workspace = root
    var agent = initAgent(config)
    let blocked = agent.runCompaction("user note", ui = quietUi())
    check not blocked.didCompact
    check "not now" in blocked.message

  test "turn_start and turn_end fire around a turn":
    let root = freshDir()
    defer: removeDir(root)
    writeHook(root, ".niminal", "log-turn-start", "turn_start",
      "echo start >> turn-log.txt\necho '{}'")
    writeHook(root, ".niminal", "log-turn-end", "turn_end",
      "echo end >> turn-log.txt\necho '{}'")
    var config = loadConfig(root, root / "config.json")
    config.sessionDir = root / "sessions"
    config.workspace = root
    config.compactionEnabled = false
    config.contextWindow = 1_000_000
    let hooks = discoverHooks(root).hooks
    let provider = TestProvider(
      name: "test",
      responses: @[ProviderResponse(content: @[text("hi")])])
    var agent = Agent(config: config, provider: provider,
      session: initSession(), hooks: hooks)
    agent.session.workspace = root
    agent.session.addUserMessage("go")
    agent.runTurn(quietUi())
    check readFile(root / "turn-log.txt") == "start\nend\n"

  test "rewritten arguments reach the tool":
    let root = freshDir()
    defer: removeDir(root)
    writeHook(root, ".niminal", "force-cmd", "pre_tool_call",
      """cat >/dev/null
echo '{"arguments":{"command":"echo from-hook"}}'
""")
    var config = loadConfig(root, root / "config.json")
    config.sessionDir = root / "sessions"
    config.workspace = root
    config.compactionEnabled = false
    config.contextWindow = 1_000_000
    var seen = ""
    var reg: ToolRegistry
    reg.register(
      ToolDefinition(name: "bash", description: "b",
        inputSchema: %*{"type": "object"}),
      proc(input: JsonNode): ToolResult =
        seen = input["command"].getStr
        ToolResult(output: "ok", isError: false))
    let provider = TestProvider(
      name: "test",
      responses: @[
        ProviderResponse(content: @[
          toolUse("1", "bash", %*{"command": "echo original"})]),
        ProviderResponse(content: @[text("done")])
      ])
    var agent = Agent(config: config, provider: provider,
      session: initSession(), tools: reg, hooks: discoverHooks(root).hooks)
    agent.session.workspace = root
    agent.session.addUserMessage("go")
    agent.runTurn(quietUi())
    check seen == "echo from-hook"

suite "cli prompt args":
  test "prompt words are one-shot by default":
    let cli = parseCliArgs(["fix", "the", "parser"])
    check cli.error.len == 0
    check cli.prompt == "fix the parser"
    check not cli.interactive
    check not cli.resumeLatest

  test "--interactive keeps the REPL after a prompt":
    let cli = parseCliArgs(["-i", "do", "x"])
    check cli.interactive
    check cli.prompt == "do x"
    let again = parseCliArgs(["--interactive", "--", "-weird", "flag"])
    check again.interactive
    check again.prompt == "-weird flag"

  test "session and resume combine with a prompt":
    let cli = parseCliArgs(["--resume", "--session", "abc", "continue"])
    check cli.resumeLatest
    check cli.sessionId == "abc"
    check cli.prompt == "continue"

  test "unknown flag errors before the prompt":
    let cli = parseCliArgs(["--nope", "hello"])
    check cli.error.len > 0
    check "Unknown option" in cli.error

suite "themes":
  test "dark 256 matches historical SGR":
    check Dark256.accent == "\e[36m"
    check Dark256.panelBg == "\e[48;5;236m"
    check Dark256.selectedBg == "\e[48;5;81m"
    check Dark256.selectedFg == "\e[30m"
    check Dark256.boldAccent == "\e[1;36m"
    check Dark256.heading == "\e[1;93m"
    check Dark256.dim == "\e[2m"
    check Dark256.text == "\e[37m"
    let compiled = compileNamedTheme("dark", cd256)
    check compiled.ok
    check compiled.theme.accent == Dark256.accent
    check compiled.theme.panelBg == Dark256.panelBg

  test "truecolor hex compiles to 38;2":
    let t = compileTheme(DarkSpec, cdTrue)
    check "38;2;" in t.accent
    check "48;2;" in t.panelBg
    check t.dim == "\e[2m"
    check t.reset == "\e[0m"

  test "depth none strips color":
    let t = compileTheme(DarkSpec, cdNone)
    check t.accent.len == 0
    check t.panelBg.len == 0
    check not t.colorsOn

  test "16-color drops panels":
    let t = compileTheme(DarkSpec, cd16)
    check t.accent.len > 0
    check t.panelBg.len == 0
    check t.selectedBg.len == 0

  test "json theme loads required tokens":
    let root = freshDir()
    defer: removeDir(root)
    createDir(root / ".niminal" / "themes")
    writeFile(root / ".niminal" / "themes" / "seafoam.json", """
{
  "name": "seafoam",
  "colors": {
    "accent": "#7fdbca",
    "success": "#9ece6a",
    "error": "#f7768e",
    "warning": "#e0af68",
    "muted": 242,
    "dim": 240,
    "text": "",
    "heading": "#c0caf5",
    "model": "#bb9af7",
    "panelBg": "#1a1b26",
    "selectedBg": "#7fdbca",
    "selectedFg": "#1a1b26"
  }
}
""")
    let loaded = findUserTheme("seafoam", root)
    check loaded.ok
    check loaded.spec.name == "seafoam"
    let compiled = compileNamedTheme("seafoam", cdTrue, root)
    check compiled.ok
    check compiled.theme.name == "seafoam"
    check "38;2;" in compiled.theme.accent
    check "seafoam" in listThemeNames(root)

  test "json rejects missing token and unknown name":
    let bad = parseThemeJson(parseJson("""{"name":"x","colors":{"accent":"#fff"}}"""))
    check not bad.ok
    check "missing" in bad.err
    let root = freshDir()
    defer: removeDir(root)
    let unknown = compileNamedTheme("nope", cd256, root)
    check not unknown.ok

  test "/theme parse and suggestions":
    check parseSlash("/theme").kind == slTheme
    check parseSlash("/theme").arg.len == 0
    check parseSlash("/theme dark").kind == slTheme
    check parseSlash("/theme dark").arg == "dark"
    check parseSlash("/theme dark extra").kind == slError
    check "/theme dark" in commandSuggestions("/theme ")
    check "/theme light" in commandSuggestions("/theme li")

  test "config theme field loads and persists":
    let root = freshDir()
    defer: removeDir(root)
    createDir(root / ".niminal")
    writeFile(root / ".niminal" / "config.json", """{"theme":"light","default_provider":"openrouter"}""")
    let cfg = loadConfig(root)
    check cfg.theme == "light"
    var patched = cfg
    patched.theme = "dark"
    persistModel(patched)
    let again = loadConfig(root)
    check again.theme == "dark"

