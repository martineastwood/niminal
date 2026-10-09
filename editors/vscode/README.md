# niminal for VS Code

Highlighting for `.nml` files, and live evaluation against a running daemon.

## Try it

From the repo root:

```
cd editors/vscode && npm install && node install.js   # then reload VS Code (--cursor for Cursor)
niminal daemon          # or "niminal: Start Daemon in a Terminal"
```

Open `examples/groove.nml`, then:

| Key (mac / other) | Does |
| --- | --- |
| Cmd+Enter / Ctrl+Enter | Evaluate the selection, or the block (lines up to the nearest blank line, widened to balance brackets) |
| Cmd+Shift+Enter / Ctrl+Shift+Enter | Evaluate the whole file |
| Cmd+. / Ctrl+. | Hush |

The **niminal** side bar lists your scenes (click one, or press Cmd+1 to Cmd+9 for the Nth, to launch it) and your tracks, with what each is playing and buttons to mute, solo and stop. Cmd+1..9 replace VS Code's group-switching keys while a niminal file has focus.

Compile errors appear as squiggles at the right place, the status bar shows the bar, beat, tempo, voices and changes waiting to land, and `niminal: Panic` is in the command palette.

Settings: `niminal.port`, `niminal.token`, `niminal.quantize` (for example `next 4 bars`), `niminal.path`.

## Tests

`npm test` runs the block finder and the protocol client against a real daemon (build it first with `cargo build`).
