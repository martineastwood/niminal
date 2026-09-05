## Mode-blind sink for agent turns (console or TUI).
## Agent talks only to TurnSink; adapters own presentation details.

import nimgent
import term
import ../session
import console
import tui

type
  MsgLevel* = enum
    mlPlain, mlWarn, mlOk, mlDim, mlError

  TurnSink* = object
    emit*: proc (level: MsgLevel, text: string) {.closure.}
    render*: proc () {.closure.}
    onChange*: proc () {.closure.}
    ## Clear any in-flight stream overlay; show assistant text when `final`
    ## (end of turn) or when the UI always surfaces mid-tool text (TUI).
    commitGenerate*: proc (response: ProviderResponse, final: bool) {.closure.}
    toolStart*: proc (call: ContentBlock) {.closure.}
    toolResult*: proc (output: string, isError: bool) {.closure.}
    poll*: proc () {.closure.}
    wasInterrupted*: proc (): bool {.closure.}
    noteInterrupted*: proc () {.closure.}
    showSession*: proc (session: Session) {.closure.}
    generate*: proc (provider: Provider,
                     request: ProviderRequest): ProviderResponse {.closure.}

proc noop() = discard

proc consoleSink*(): TurnSink =
  var lastCall: ContentBlock
  proc emit(level: MsgLevel, text: string) =
    case level
    of mlPlain: echo text
    of mlWarn: echo text.color(cYellow)
    of mlOk: echo text.color(cGreen)
    of mlDim: echo text.color(cDim)
    of mlError:
      echo "PROVIDER_FAILED".color(cRed)
      echo text.color(cRed)
  proc show(session: Session) =
    if session.events.len == 0:
      echo "Started a new session: " & session.id
    else:
      echo "Resumed session: " & session.id
      echo "Events: " & $session.events.len
  TurnSink(
    emit: emit,
    render: noop,
    onChange: noop,
    commitGenerate: proc (response: ProviderResponse, final: bool) =
      if final: printResponse(response),
    toolStart: proc (call: ContentBlock) =
      lastCall = call
      printToolStart(call),
    toolResult: proc (output: string, isError: bool) =
      printToolResult(output, isError, lastCall),
    poll: noop,
    wasInterrupted: proc (): bool = false,
    noteInterrupted: noop,
    showSession: show,
    generate: proc (provider: Provider, request: ProviderRequest): ProviderResponse =
      provider.generate(request)
  )

proc tuiSink*(tui: ptr TUI, footer: proc (): string {.closure.}): TurnSink =
  ## `footer` supplies status-bar text (typically agent.statusFooter).
  var cancelled = false
  proc emit(level: MsgLevel, text: string) =
    let line = case level
      of mlPlain: text
      of mlWarn: "\e[33m" & text & "\e[0m"
      of mlOk: "\e[32m" & text & "\e[0m"
      of mlDim: "\e[2m" & text & "\e[0m"
      of mlError: "\e[31m" & text & "\e[0m"
    if level == mlError:
      tui[].addLine("\e[31mPROVIDER_FAILED\e[0m")
    tui[].addLine(line)
  TurnSink(
    emit: emit,
    render: proc () = tui[].render(),
    onChange: proc () = tui[].setFooter(footer()),
    commitGenerate: proc (response: ProviderResponse, final: bool) =
      tui[].finishThinking()
      if final:
        tui[].finishStream(response)
        tui[].setFooter(footer())
      else:
        tui[].discardStream()
        tui[].addAssistantMarkdown(response.textContent()),
    toolStart: proc (call: ContentBlock) =
      tui[].addToolStart(call),
    toolResult: proc (output: string, isError: bool) =
      tui[].addToolResult(output, isError),
    poll: proc () = tui[].checkInterrupt(),
    wasInterrupted: proc (): bool =
      tui[].wasInterrupted or tui[].shouldExit or cancelled,
    noteInterrupted: proc () =
      tui[].addLine("\e[33mInterrupted\e[0m"),
    showSession: proc (session: Session) =
      tui[].loadSessionView(session),
    generate: proc (provider: Provider, request: ProviderRequest): ProviderResponse =
      cancelled = false
      var started = false
      var req = request
      req.wakeFd = interruptFd()
      try:
        result = provider.generateStream(req, proc (ev: StreamEvent): bool =
          case ev.kind
          of seThinkingDelta:
            tui[].appendThinking(ev.text)
            discard tui[].pollBusy(0)
            if tui[].wasInterrupted or tui[].shouldExit:
              cancelled = true
              return false
            tui[].render()
            true
          of seTextDelta:
            tui[].finishThinking()
            if not started:
              tui[].beginStream()
              started = true
            tui[].appendStream(ev.text)
            discard tui[].pollBusy(0)
            if tui[].wasInterrupted or tui[].shouldExit:
              cancelled = true
              return false
            tui[].render()
            true
          of seToolCallDelta:
            tui[].finishThinking()
            discard tui[].pollBusy(0)
            if tui[].wasInterrupted or tui[].shouldExit:
              cancelled = true
              return false
            tui[].render()
            true
          of seFinished:
            true
          of seWake:
            discard tui[].pollBusy(0)
            if tui[].wasInterrupted or tui[].shouldExit:
              cancelled = true
              return false
            tui[].render()
            true
        )
        if cancelled:
          tui[].cancelStream()
      except CatchableError:
        tui[].discardStream()
        raise
  )
