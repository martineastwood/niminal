Apple Silicon macOS alpha (macOS 13 or newer).

Download **Niminal-<version>-macos-arm64.zip** below. It includes Tracker, Stage, the command-line tools, the optional VS Code extension, and examples. No Rust, Node.js, or Homebrew installation is needed.

### Install and play

1. Unzip the download and copy the entire **Niminal** folder into `/Applications`.
2. Start the audio engine in Terminal:
   ```sh
   /Applications/Niminal/bin/niminal server
   ```
   Leave this terminal running while playing.
3. Open **Niminal Tracker.app** or **Niminal Stage.app** from `/Applications/Niminal` and connect to the engine.

Read **Getting Started.txt** inside the download for editor setup, examples, offline rendering, and upgrades. Copy examples into a writable project folder before editing them.

This alpha is ad-hoc signed and **not notarized by Apple**. If macOS blocks an app or tool, attempt to open it, then approve that specific item under **System Settings → Privacy & Security → Open Anyway** if you trust the download.

Syntax and project formats may change. Keep backups of your work. Include `BUILD.json` when [reporting an issue](https://github.com/martineastwood/niminal/issues).

The `.sha256` asset contains the ZIP's SHA-256 checksum.
