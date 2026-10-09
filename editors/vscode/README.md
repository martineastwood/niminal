# niminal for VS Code

Highlighting for `.nml` files, and live evaluation against a running daemon.

## Try it

From the repo root, with the extension folder open in VS Code (or run `code --extensionDevelopmentPath=editors/vscode examples`):

```
cd editors/vscode && npm install
niminal daemon          # or "niminal: Start Daemon in a Terminal"
```

Open `examples/groove.nml`, then:

| Key (mac / other) | Does |
| --- | --- |
| Cmd+Enter / Ctrl+Enter | Evaluate the selection, or the block (lines up to the nearest blank line, widened to balance brackets) |
| Cmd+Shift+Enter / Ctrl+Shift+Enter | Evaluate the whole file |
| Cmd+. / Ctrl+. | Hush |

Compile errors appear as squiggles at the right place, the status bar shows the bar, beat, tempo, voices and changes waiting to land, and `niminal: Panic` is in the command palette.

Settings: `niminal.port`, `niminal.token`, `niminal.quantize` (for example `next 4 bars`), `niminal.path`.

## Tests

`npm test` runs the block finder and the protocol client against a real daemon (build it first with `cargo build`).
