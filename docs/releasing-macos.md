# Making the macOS alpha download

The alpha is one ZIP containing a `Niminal` folder. Users copy that folder to
`/Applications/Niminal`, add its `bin` directory to their shell PATH, and run
`niminal server` before opening a client. Both desktop apps are optional, as is
the VSIX. There is no installer, background service, automatic update or Apple
Developer account dependency. The apps and CLI tools are ad-hoc signed and are
not notarized.

## Build locally

Use an Apple Silicon Mac with Xcode command-line tools, stable Rust, Node.js 24
and Python 3.11 or newer. Build from a clean, committed checkout:

```sh
scripts/check.sh --test
python3 scripts/package-macos.py 0.0.1-alpha.1
```

The script installs locked npm dependencies, builds both CLI tools and both
Tauri apps, packages the extension with its runtime dependencies, signs the
executables, smoke-tests the staged download, and creates:

```text
releases/Niminal-0.0.1-alpha.1-macos-arm64.dmg
releases/Niminal-0.0.1-alpha.1-macos-arm64.dmg.sha256
```

It shares Rust build outputs under `target/macos-alpha` and overrides
`target-cpu=native` with the M1 baseline. The deployment target is macOS 13.
Frontend build output also stays under `target`, leaving checked-in `dist`
files alone. CLI version strings, app bundle versions and `BUILD.json` identify
the requested release. The VSIX retains its separate extension version.
`BUILD.json` also records the commit and whether sources were uncommitted.

Use `--allow-dirty` only for a local preview. The script refuses to overwrite an
existing ZIP; move the preview aside or choose another version before rebuilding.
Nothing is uploaded or published by this script.

The **macOS alpha package** GitHub Actions workflow runs the engine/sequencer
checks, frontend and extension tests, and desktop Rust backend tests before
building. Run it manually with a version, or push a `v0.0.1-alpha.1`-style tag.
It uploads the ZIP and checksum as workflow artifacts; it does not create a
public release. Before the first release, exercise the workflow on the intended
commit so its output comes from a clean runner.

The MIDI clock's exact scheduling and stale-tick rules run as deterministic unit
tests in CI. The virtual-MIDI arrival-time test needs an idle machine: shared
runners cannot guarantee its 5ms scheduler tolerance. Run it explicitly when
checking MIDI timing locally:

```sh
(cd sequencer && cargo test -p niminal-live-cli --lib clock::tests::messages_leave_when_their_time_comes -- --ignored)
```

## Download contents

```text
Niminal/
  Niminal Tracker.app
  Niminal Stage.app
  bin/niminal
  bin/niminal-live
  niminal-vscode.vsix
  Examples/
  Getting Started.txt
  LICENSE
  BUILD.json
```

Stage finds tools beside the app bundle, then in `/Applications/Niminal/bin`,
then developer installations. The extension honors explicit path settings and
otherwise looks in `/Applications/Niminal/bin`, PATH, and `~/.cargo/bin`. Its
language server works without a running engine. Stage shows a startup hint when
no engine is listening; the existing Connect button retries.

## Verify before publishing

Download the ZIP through a browser on a second Mac or clean account without
Rust, Node.js, a checkout or older Niminal binaries. This is important: local
builds do not reproduce downloaded-file quarantine or a fresh user's PATH.

- Check the SHA-256, open the dmg, and follow `Getting Started.txt` exactly.
- Exercise the normal Privacy & Security approval flow for both apps and CLI
  tools. Keep quarantine intact; do not disable Gatekeeper to pass this check.
- Verify CLI startup and offline rendering before starting the engine.
- Open each client before the engine, check its helpful error, then start the
  engine and connect. Confirm multiple clients can connect concurrently.
- Install the VSIX in Finder-launched VS Code. Check diagnostics with no engine,
  then live evaluation with the engine. Test a tool path containing spaces.
- In Tracker and Stage, play, stop, save, reopen and export a small project.
  Import a sample and reopen the project with the original sample moved away.
- Test Stage's replay/gallery helpers with the packaged tools.
- Replace the complete application folder with another build, reload the
  extension, and check that externally saved projects and samples remain intact.
- Confirm the advertised macOS minimum on that OS, or narrow the support claim
  to the oldest OS actually validated. Also check Intel Macs are clearly excluded.

To re-run the automated artifact checks after copying the folder out of the dmg:

```sh
python3 scripts/verify-macos-package.py /path/to/Niminal
```

That developer smoke test needs Python and Node; users do not. It checks code
signatures, architecture, version strings, VSIX runtime dependencies, offline
rendering and LSP startup. It cannot validate Gatekeeper approvals, Finder UI or
physical audio/MIDI devices.

## Publish after review

Create a GitHub prerelease for the tested tag and attach the ZIP and checksum.
Describe it as an experimental Apple Silicon alpha, state the tested macOS
versions, and mention that it is ad-hoc signed rather than notarized. Include the
server-first startup instructions. Update the README download link when the
release exists. Release archives are immutable: use a new alpha version for
fixes. Windows, Linux, Homebrew and notarization can be added later using the
same versioned-release approach.
