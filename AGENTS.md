# Working in niminal

niminal is a live-coding audio language with its own DSP engine, written in Rust. **Stage** (`stage/`) is the app: the engine and the
live sequencer run inside it. The **`niminal`** command (`crates/niminal-cli`) plays, renders and serves the same engine without a
window. A **VS Code extension** (`editors/vscode/`) evaluates code in Stage and gets its editing help from `niminal lsp`.

This is a greenfield project, so breaking changes are allowed and preferable to shimming legacy behaviour. Keep it simple: no
unnecessary abstractions or over-engineering.

## Where things are

One Cargo workspace at the root holds every crate (`crates/*`). Stage is its own manifest (`stage/src-tauri`, it needs the desktop
toolchain) and the extension and Stage's window are Node projects. Build output goes to `target/` (one directory for the workspace).

| Crate | What it is | Start reading at |
| --- | --- | --- |
| `niminal-engine` | DSP: graph, voices, mixer, every opcode (filters, oscillators, delays, spectral, ...) | `src/ops/mod.rs`, `src/mixer.rs` |
| `niminal-score` | Values with units, events, the `.nms` section file | `src/model.rs` |
| `niminal-pattern` | Patterns: values over cycles of rational time, mini-notation, transforms | `src/lib.rs` |
| `niminal-lang` | The `.nml` language for sound: parser, unit checker, two compilers (`lower/` to graphs, `kernel/` to fused kernels), the std shelves in `std/*.nml` | `src/lib.rs`, `src/opcodes/mod.rs` |
| `niminal-server` | The engine as a service: sessions, definitions, timestamped events, routing, the real-time renderer, the limiter | `src/engine_server.rs`, `src/session/` |
| `niminal-ws` | JSON-RPC over WebSocket (server and client); crash log and the VS Code install offer | `src/lib.rs` |
| `niminal-compose` | The sequencer's half of the language: clips, scenes, patterns, commands, arrangements | `src/perform/`, `src/schedule.rs` |
| `niminal-host` | Runs the engine inside a program: sound card, render threads, session log, replay (`host::Host`) | `src/host.rs` |
| `niminal-sequencer` | The live sequencer: tempo, quantized changes, keyboards/MIDI/OSC, the API that Stage serves (`protocol.rs`) | `src/live/mod.rs`, `src/protocol.rs` |
| `niminal-lsp` | Language server: diagnostics, completion, hover | `src/lib.rs` |
| `niminal-cli` | The `niminal` binary: `play`, `render`, `server`, `replay`, `explain`, `lsp`, `devices`, `docs` | `src/main.rs`, `src/play.rs`, `src/engine_commands.rs` |

The sound side (`engine`, `lang`, `server`) never depends on the sequencer side (`pattern`, `compose`, `sequencer`): keep it that way.
Layers, bottom to top: `engine`, `score` and `pattern` depend on nothing in the workspace; `lang` on `engine` and `score`; `server` on those
and `lang`; `ws` on `server`; `compose` on `lang` and `pattern`; `host` and `sequencer` on `server` and `ws` (`sequencer` also on `compose`);
`lsp` on `compose`; `cli` and Stage on nearly everything.

Other directories: `stage/` (Tauri app: `src-tauri/` Rust, `src/` TypeScript window), `editors/vscode/` (extension, plain JS),
`editors/codemirror/` (shared editor bits Stage uses), `clients/js/` (JavaScript client of the engine API; the extension builds on
it), `examples/` (`.nml` pieces, also the audio baseline), `docs/` (the niminal.dev site, Astro), `scripts/` (check, install, package).

## The fast feedback loop

Run the smallest thing that covers your change, then the whole check once before you finish. Times are warm builds.

| Changed | Run | Time |
| --- | --- | --- |
| anything in Rust | `cargo clippy --workspace --all-targets -- -D warnings` (or `scripts/check.sh`) | ~10 s |
| `niminal-engine` | `cargo test -p niminal-engine` | ~2 s |
| `niminal-lang` | `cargo test -p niminal-lang` (one file: `--test stdlib`) | ~20 s |
| `niminal-server` | `cargo test -p niminal-server` | ~15 s |
| `niminal-compose` / `-pattern` / `-lsp` | `cargo test -p niminal-compose` etc. | ~5 s |
| `niminal-sequencer` | `cargo test -p niminal-sequencer` | ~20 s |
| `niminal-cli` (renders every example) | `cargo test -p niminal-cli` | ~30 s |
| Stage or the extension | `scripts/check.sh --apps --test` (adds Stage's clippy, cargo test, tsc, vitest, and the extension's tests) | ~1 min |
| before finishing | `scripts/check.sh --test` (or `--test --apps`) | ~2-3 min |

`scripts/check.sh` options combine: `--test` runs the tests, `--apps` adds Stage and the extension. Then install with
`scripts/dogfood.sh` (default release profile, ~20 s) and restart any running `niminal server`/`niminal play`; they keep the old build
until restarted. `niminal` and Stage each carry their own copy of the language, so rebuild Stage too
(`cd stage && npm run tauri -- build --bundles app`). Only use `--profile release-fast` (10+ minutes) for proper releases.

Why `scripts/dogfood.sh`: the slow compile blocks iteration and the performance difference doesn't matter during development. CI
fails on clippy lints and on tests that stopped compiling; both are caught locally in seconds.

## Checking a change is safe to ship

Niminal runs live and drives real speakers, so a change is not done until `scripts/check.sh --test` passes. A change to Stage or the
extension also needs `--apps`. Do not skip the tests because a change looks small.

These tests are the safety net. Never delete, loosen or `#[ignore]` them to make a change pass; if one fails, the change is wrong
until proven otherwise:
- `crates/niminal-engine/tests/master.rs`: the limiter holds its ceiling (including between samples), non-finite input becomes silence, DC is removed.
- `crates/niminal-lang/tests/stability.rs` and `crates/niminal-server/tests/stability.rs`: the std instruments and effects stay finite under extreme controls, an effect silenced for going wrong plays again once redefined, and a default session cannot exceed the limiter ceiling.
- `crates/niminal-lang/tests/numeric_guards.rs`: maths with no real answer gives 0, never NaN.
- `crates/niminal-cli/tests/baseline.rs` and `tempo_ramp.rs`: the audio of every example and a scripted live session, and no note lost where the tempo changes.

Why: a bug that blows up live, or makes a loud noise, is the one kind we cannot ship. The suite is quick enough (the engine and its
dependencies are optimised even in test builds) that running all of it each time costs little.

How to apply:
- No `.unwrap()` in non-test code (clippy's `unwrap_used` is on for the workspace and Stage; tests may unwrap). Return an error, or use `.expect("why this cannot fail")` for a true invariant. A panic on the live path drops the set.
- The limiter must stay on in everything that plays live. `without_limiter` is for tests and offline renders only; do not add a way to turn it off on the live path.
- When a new opcode, instrument or effect is added, add it to the extreme-controls coverage (a shelf preset is picked up automatically) and make sure it cannot produce NaN or infinity from any input.
- A change that moves audio on purpose: check how far it moved before re-recording with `BLESS=1 REFERENCE=1 cargo test -p niminal-cli --test baseline`, and say how far in the commit. The first run without those variables prints how far each example moved.
- When you change a shelf file under `crates/niminal-lang/std/` the stdlib tests check that every preset still compiles on its own.
- Language files can say which versions they are for (`niminal 0.1` on its own line). Raise `LANGUAGE_VERSION` in `crates/niminal-lang/src/version.rs` only when a change would break files written for the version before, and say what changed in the commit.

## Code layout for navigation

Big modules are directories split by concern, not single files. Start at the `mod.rs` (types, entry points), then open the file named for the concern:
- `crates/niminal-lang/src/lower/`: `entry` (compile controls/instruments), `stmt`, `expr`, `lists`, `arith`, `calls`, `io`, `user_fn`, `tables`, `signals`, `sampler`, `chain`, `check`.
- `crates/niminal-lang/src/kernel/`: `emit`, `functions`, `stmts`, `exprs`, `scopes`, `calls`, `natives`, `lanes`, `helpers`.
- `crates/niminal-lang/src/compile/`: `nodes`, `samples`, `config`, `literals`, `tests`.
- `crates/niminal-lang/src/parser/`: `input`, `items`, `stmts`, `exprs`, `env`, `helpers`, `tests`.
- `crates/niminal-lang/src/opcodes/`: `mod.rs` (registry) and `granular.rs`.
- `crates/niminal-compose/src/perform/`: `values`, `ops`, `clip_ops`, `args`, `clips`, `commands`, `grid`, `music`, `convert`.
- `crates/niminal-server/src/session/`: `mod.rs` (state and definitions), `input.rs` (events), `realtime.rs` (the audio-thread renderer).
- `stage/src-tauri/src/`: `session.rs` (the engine, the sequencer, the sockets, audio settings), `main.rs` (Tauri commands), `audio.rs`, `settings.rs`.

Methods of one struct (`Lower`, `Kb`, `Parser`, `Perform`) are spread over several `impl` blocks, one per file; cross-file methods are `pub(super)`. Prefer adding to the file that matches the concern over growing `mod.rs`, and split again if a file passes about 1000 lines. The largest files now are `crates/niminal-compose/src/tests.rs`, `crates/niminal-sequencer/src/live/mod.rs`, `crates/niminal-server/src/session/realtime.rs` and `.../input.rs`, `crates/niminal-compose/src/schedule.rs`, `crates/niminal-engine/src/mixer.rs` and `crates/niminal-lsp/src/lib.rs` (1500-3300 lines): search them rather than reading them whole.

## Things that surprise people

- Stage is not part of `cargo ... --workspace`: it has its own manifest (`stage/src-tauri`) and needs the Tauri prerequisites. Use `scripts/check.sh --apps`.
- On this machine every build prints a `rust-objcopy ... libLLVM.dylib ... SIGABRT` warning. It is harmless noise from the toolchain; look past it for real errors.
- `docs/src/content/docs/reference/opcodes.md` is generated by `niminal docs`; don't edit it.
- `AGENTS.md` is the only agent instruction file. `design/`, `goals.md` and `TODO.md` are local notes that are gitignored, so they may be missing and may be out of date: trust the code and `docs/`.
- The Tracker (a pattern-grid app) lives on the `tracker` branch, not on `main`.
- Takes and sessions are logged to `~/.niminal/sessions`; panics go to `~/.niminal/crash.log`; Stage remembers its sound card and ports in `~/.niminal/stage.json`.
