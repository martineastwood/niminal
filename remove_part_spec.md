# Spec: remove `part` from the language

Greenfield project (see AGENTS.md): breaking changes are preferred over shims. Keep it simple, no new abstractions. After the change, rebuild and install for dogfooding: `cargo install --path crates/niminal-cli` and `cargo install --path sequencer/nimlive` (default release profile, not `release-fast`). Tests: `cargo test -p niminal-lang` from the repo root; `cd sequencer && cargo test --workspace`.

## Already done (do not redo)

- `.fit(...)` removed. `sample(...).beats(N)` and `.duration(T)` now always fit the loop to that length (`crates/niminal-lang/src/compile.rs`, `compile_sample`). Docs, tests and `examples/breakbeat.nml` updated.
- `examples/breakbeat.nml` now uses the clip form: `clip chop { instrument: chops, notes: straight }` with `chop.start`, and named grid/pattern bindings. This already works with no language change. Verified: sample/kit/slices as a clip `instrument:`, and `notes: some_grid_binding`.

## The model we are moving to

| Concept | What it is |
| --- | --- |
| `instr`, `sample`, `kit`, `.slices(n)` | Instruments: make sound per note. Routing to buses/processors is written inside the instr (`space += out`). Voice limits (`max`, `steal`, `mono`) and `quantize` are instr settings. |
| `bus`, `processor` | Long-running DSP. |
| `clip` | An instrument plus lanes (notes, grid, params such as `gain:`, `pan:`, `bright:`) on a timeline. Started, stopped, muted, soloed, transformed live **by clip name**. |
| `scene` | A set of clips started together; starting it stops all other running clips. |

There is no user-visible `part`. Gain and pan are ordinary lanes the user wires up in their instr. A clip has no `output:`, `gain:` or `pan:` setting of its own beyond being lanes. Grids and patterns are bindings defined outside the clip and referenced as `notes: my_grid`.

`part` was a "track slot" that a clip plays on, owning mute/solo, mix, output routing, voice limits and live overrides. All of that moves onto the clip (by name) or the instr. The compiler may keep its internal node/slot concept (`Action::Play { node }`, `nodes`) — only the surface language and docs change. Accepted loss: continuity across a scene swap (e.g. a `mono` bass fading the previous clip's last note when a different clip takes over, and mute state surviving a swap).

## Work

1. **Parser/AST** (`sequencer/compose/src/parser.rs`, `ast.rs`): remove the `part` item, both block (`part x { instrument = y }`) and one-line (`part x = y(...)`) forms. Give a clear error for `part` pointing to `clip x { instrument: y }`. Remove `Item::Part`.
2. **Clips with a bare instrument** (`sequencer/compose/src/compile.rs`, `implicit_nodes` ~line 506): the implicit-node rule currently only applies to names declared with `instr` (`Item::Instr`). Extend it to `sample`, `kit` and `.slices(n)` bindings so `chops.play(...)` and `scene { chops: [...] }` work with no clip. (`implicit_nodes` is how `pluck.play([c2])` works today.)
3. **Settings that lived on `part`**:
   - `max`, `steal`, `mono`, `quantize`: confirm they are accepted on `instr` and in the instrument call (`pluck(bright: 0.2)`); docs say instr already supports them (`reference/language/instruments.md`, ~line 159). Voices are counted per node; keep counting per clip name.
   - `notes = it.thin(10%)` part transform: drop; users write `clip.thin(10%)` or transform in the clip's `notes:`.
   - `output`, `gain`, `pan`: drop from the language. Check every use in examples/tests and move routing into the instr.
4. **Scenes** (`scene`): entries reference clips (`scene verse { bass, lead }`, `scene chorus { bass: bass.transpose(+5st), lead: answer }`). Keep the `name: expr` form; the name must now resolve to a clip or an instrument (implicit clip), not a part. Keep "starting a scene stops every other running clip".
5. **Live commands** (`start`, `stop`, `mute`, `unmute`, `solo`, `unsolo`, `.play(x)`, `.reverse`, `.set(...)`, `.undo`, `.clear`): resolve against clip names. `x.play(pattern)` on an instrument or clip name is an anonymous clip.
6. **Other files that reference `Part`** (from `grep Item::Part|Part`): `crates/niminal-lang/src/program.rs`, `crates/niminal-server/src/midi_out.rs`, `sequencer/compose/src/{analysis,ast,compile,dsp,ide,modules,parser}.rs`, `sequencer/lsp/src/{complete,features,lib}.rs`. Remove `part` from completions, hover, symbols and the VS Code grammar/extension (search `vscode*/` and any `*.tmLanguage*`, `language-configuration`) — I did not check the extension.
7. **Examples to rewrite to clips** (all use `part`): `examples/song.nml`, `drums.nml`, `processors.nml`, `room.nml`, `midi_osc.nml`, `live_controls.nml`, `lsp_tour.nml`. `song.nml` runs two parts off one instrument with different gain (`part hats { instrument = drums(gain: -8db) }`): make these two clips with `instrument: drums` and `instrument: drums(gain: -8db)` — verify that renders the same. For parts with `output = some_processor`, move the routing into the instr (e.g. `proc += out`) or use a bus send.
8. **Docs**: delete the "Parts" section of `docs/src/content/docs/reference/language/performance.md`, rewrite the intro ("built around clips, scenes, and parts") and the "Parts: clips with an instrument" heading; update `routing.md` ("Instruments, parts and processors", `part lead { output = echo }`, `part dry`/`part wet` examples ~lines 6-90), `samples.md` (~lines 44-54: `part drums { instrument = chops }`, `part snare { instrument = drums.snare; output = snare_audio }`), `instruments.md` (~159-169 "Voices are counted per part"), and any other `part` mention (`grep -rn "part " docs/`).
9. **Tests**: update tests that declare `part` (`sequencer/compose/src/tests.rs`, notably `a_node_can_be_one_line_and_an_instrument_named_as_a_part_gets_one` ~line 2105, plus the ones asserting node counts around lines 617, 1625, 2122); add tests: sample/kit/slices playable by name with no clip; `part` gives the migration error; scene referencing clips stops others; `song.nml` still renders.
10. **Verify**: run all tests; render every example (`nimlive render examples/X.nml --out /tmp/x.wav --bars 4`, use the scratchpad dir not /tmp if available) and confirm none error and none are silent; reinstall binaries.

## Decisions settled while implementing

- Two clips using the same instrument play independently. Render tests confirm that both `mono` and `max = 1` apply separately per clip name.
- Voice policies (`max`, `steal`, `mono`) and `quantize` stay on instrument definitions. Instrument calls supply note parameter defaults.
- Samples, kit members and sliced samples can be called inside an `instr`. One instrument can wrap the whole kit with `instr drum_voice(sample) { out = drums(sample: sample) }`; forwarding `sample` preserves member names and choke groups. Separate wrappers are optional for different routing or processing per drum.
- Explicit `gain: db` and `pan: deg` parameters are wired inside the instrument and can affect bus sends before processing.
- The live API uses `status.clips` for playback state, `status.defined_clips` for definition names, and `clip` in edit requests. Quantization configuration uses `clip:`.
- Bare-instrument live commands create persistent implicit clips, so later evaluations retain their playback slots.
