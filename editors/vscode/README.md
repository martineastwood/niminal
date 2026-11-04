# niminal for VS Code

Highlighting for `.nml` files and `.nms` score files (JSON with units, note names and mini-notation highlighted inside strings), language-server editing help, and live evaluation through Niminal Stage.

Full user docs: [niminal.dev](https://niminal.dev) · [Editor and LSP](https://niminal.dev/reference/editor/)

## How it works with Stage

Niminal Stage has the audio engine and the sequencer inside it. The extension sends the code you evaluate to
Stage, which plays it, so the clips and scenes you evaluate appear in Stage's lanes and scene buttons and play
on its transport, with its meters. If Stage is not running, **niminal: Connect** (or evaluating) offers to open it.

Editing support (errors, completion, hover) comes from `niminal lsp`: the `niminal` command, which comes in the same download as Stage. The extension finds it in `/Applications/Niminal/bin` (or on your PATH, or use the `niminal.path` setting), and it works without Stage running.

Choose **Extensions: Install from VSIX...**, select `niminal-vscode.vsix`, then reload the editor.

## Try it

From the repo root:

```
cd editors/vscode && npm install && node install.js          # VS Code
cd editors/vscode && npm install && node install.js --cursor # Cursor
# then reload the editor, and open Niminal Stage (or let the extension open it)
```

The sidebar launch / mute / solo / stop buttons use the live sequencer API that Stage serves (`launch`, `mute`, …) via the [`live-client.js`](live-client.js), built on the generic [`niminal-client`](../../clients/js).

Open `examples/groove.nml`, then:

| Key (mac / other) | Does |
| --- | --- |
| Cmd+Enter / Ctrl+Enter | Evaluate the selection, or the block (lines up to the nearest blank line, widened to balance brackets) |
| Cmd+Shift+Enter / Ctrl+Shift+Enter | Evaluate the whole file |
| Cmd+. / Ctrl+. | Hush |
| Cmd+1..9 | Launch the Nth scene (while a `.nml` file has focus) |

The **niminal** side bar lists your scenes (click one, or press Cmd+1 to Cmd+9 for the Nth, to launch it) with how many of their clips are playing, and your clips, with what has been done to each and buttons to mute, solo and stop. The status bar flags a soloed or muted clip. Cmd+1..9 replace VS Code's group-switching keys while a niminal file has focus.

Compile errors appear as squiggles at the right place, the status bar shows the bar, beat, tempo, voices and changes waiting to land, and `niminal: Panic` is in the command palette.

The extension shares Stage's sequencer (the `main` chair). Clips, scenes, lanes, transforms and quantized edits live there, so what you evaluate in the editor and what you evaluate in Stage are one performance. If both define the same name, the last one evaluated wins. The text itself stays in each editor: Stage does not show what VS Code holds.

Settings: `niminal.port` (the tools port Stage serves the sequencer on, default 7400), `niminal.token`, `niminal.quantize` (for example `next 4bars`), `niminal.stageApp` (the app to open, default `Niminal Stage`), `niminal.path` (the `niminal` command).

## Language server

The extension starts `niminal lsp` (found through the `niminal.path` setting) for editing support that works without Stage:

- errors as you type, with the compiler's own messages and help
- completion for names in the file, built-in opcodes, methods after a `.`, named arguments, waveforms and unit suffixes
- hover with a signature and documentation, and go-to-definition and an outline
- a comment directly above an `instr` or `fn` (`//` or `///`) is its documentation

"Restart Language Server" in the command palette picks up a newly installed binary.
Other editors can use it too: run `niminal lsp` as a stdio language server for `.nml` files.

## Live controls, MIDI and OSC

Named controls (`ctl`), MIDI CC / learn, OSC in, and keyboard clips (`keys: midi.keys`) are part of the language. Stage opens the MIDI inputs and clock; choose them in its toolbar. A control learned with `midi.learn` is written back into the code in the editor that asked.

See the [live coding guide](https://niminal.dev/guide/live-coding/) and [performance reference](https://niminal.dev/reference/language/performance/). Examples: `examples/live_controls.nml`, `examples/midi_osc.nml`.

## Physical-model starters

Type `instr-bowed` or `instr-reed` in a `.nml` file to insert a playable waveguide instrument with an excitation and release envelope. The shared language server supplies completion and hover for `bowed`, `reed`, and their named controls. Ready-made variants are available with `use std/instruments.{bowed_string, reed_wind}`; `examples/physical_models.nml` auditions both.

After updating, install the current `niminal` binary, run `npm run install-ext` here, and reload the VS Code window so its manifest and snippets refresh.

## Tests

`npm test` runs the block finder and the protocol client against a real engine and sequencer (build them first with `cargo build`).
