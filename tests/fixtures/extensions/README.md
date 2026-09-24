# Extension test fixtures

Minimal extensions used by `niminal_extensions_test`. Each fixture exercises a
specific part of the extension protocol:

- `fixture` — hooks, tools, notices, entries, and compaction
- `host` — host requests and UI callbacks
- `parallel` — concurrent command handling
- `status_demo` — footer status segments and clearing
- `todo_demo` — extension tools, widgets, persistence, and widget actions
- `widget_demo` — widget updates from `ui_action` messages

These are test harnesses only. User-facing examples live in the separate
`extensions_and_tools` repository.
