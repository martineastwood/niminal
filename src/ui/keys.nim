## Keys for the niminal TUI input layer.

type
  Key* = enum
    keyNone
    keyChar       ## printable; see InputEvent.ch
    keyEscape
    keyEnter
    keyBackspace
    keyDelete
    keyLeft
    keyRight
    keyUp
    keyDown
    keyHome
    keyEnd
    keyPageUp
    keyPageDown
    keyCtrlA
    keyCtrlB
    keyCtrlC
    keyCtrlE
    keyCtrlF
    keyCtrlN
    keyCtrlO
    keyCtrlP
    keyCtrlV
    keyTab
    keyShiftEnter  ## newline in the composer (Shift+Enter)
