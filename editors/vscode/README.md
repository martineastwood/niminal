# niminal for VS Code

Highlighting for `.nml` files and `.nms` score files (JSON with units, note names and mini-notation highlighted inside strings), language-server editing help, and live evaluation against a running server.

Full user docs: [niminal.dev](https://niminal.dev) · [Editor and LSP](https://niminal.dev/reference/editor/)

## Try it

From the repo root:

```
cd editors/vscode && npm install && node install.js          # VS Code
cd editors/vscode && npm install && node install.js --cursor # Cursor
# then reload the editor
niminal server          # or "niminal: Start Server in a Terminal"
```

The sidebar launch / mute / solo / stop buttons use the extension's private live sequencer API (`launch`, `mute`, …) via the [`live-client.js`](live-client.js), built on the generic [`niminal-client`](../../clients/js).

Open `examples/groove.nml`, then:

| Key (mac / other) | Does |
| --- | --- |
| Cmd+Enter / Ctrl+Enter | Evaluate the selection, or the block (lines up to the nearest blank line, widened to balance brackets) |
| Cmd+Shift+Enter / Ctrl+Shift+Enter | Evaluate the whole file |
| Cmd+. / Ctrl+. | Hush |
| Cmd+1..9 | Launch the Nth scene (while a `.nml` file has focus) |

The **niminal** side bar lists your scenes (click one, or press Cmd+1 to Cmd+9 for the Nth, to launch it) and your tracks, with what each is playing and buttons to mute, solo and stop. Cmd+1..9 replace VS Code's group-switching keys while a niminal file has focus.

Compile errors appear as squiggles at the right place, the status bar shows the bar, beat, tempo, voices and changes waiting to land, and `niminal: Panic` is in the command palette.

The extension owns a separate live sequencer process. Clips, scenes, lanes, transforms and quantized edits live there; it sends generic events to the server. Each editor session has its own performance state.

Settings: `niminal.port` (generic server, default 7401), `niminal.token`, `niminal.quantize` (for example `next 4bars`), `niminal.path`.

## Language server

The extension starts `nimlive lsp` (the `nimlive` binary, found through the `niminal.livePath` setting) for editing support that works without a running server:

- errors as you type, with the compiler's own messages and help
- completion for names in the file, built-in opcodes, methods after a `.`, named arguments, waveforms and unit suffixes
- hover with a signature and documentation, and go-to-definition and an outline
- a comment directly above an `instr` or `fn` (`//` or `///`) is its documentation

"Restart Language Server" in the command palette picks up a newly installed binary.
Other editors can use it too: run `nimlive lsp` as a stdio language server for `.nml` files.

## Live controls, MIDI and OSC

Named controls (`ctl`), MIDI CC / learn, OSC in, and keyboard clips (`keys: midi.keys`) are part of the language. Set `niminal.midi` (empty string for all inputs) and/or `niminal.oscPort` for the editor’s live sequencer, then reload the editor. For terminal performances with hardware or OSC, run `nimlive live --midi` and/or `--osc-port` alongside the server.

See the [live coding guide](https://niminal.dev/guide/live-coding/) and [performance reference](https://niminal.dev/reference/language/performance/). Examples: `examples/live_controls.nml`, `examples/midi_osc.nml`.

## Tests

`npm test` runs the block finder and the protocol client against a real server (build it first with `cargo build`).
