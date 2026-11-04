Apple Silicon macOS alpha (macOS 13 or newer).

Download **Niminal-<version>-macos-arm64.dmg** below. It includes Stage, the `niminal` command, the optional VS Code extension, and examples. No Rust, Node.js, or Homebrew installation is needed.

### Install and play

1. Open the dmg and drag the entire **Niminal** folder onto the Applications shortcut.
   With Homebrew instead: `brew tap martineastwood/niminal && brew trust --cask martineastwood/niminal/niminal && brew install --cask martineastwood/niminal/niminal`.
2. Open **Niminal Stage.app** from `/Applications/Niminal`. The audio engine is inside it.

Read **Getting Started.txt** inside the download for editor setup, examples and upgrades. Copy examples into a writable project folder before editing them.

This alpha is ad-hoc signed and **not notarized by Apple**. If macOS blocks Stage or `niminal`, attempt to open it, then approve that specific item under **System Settings → Privacy & Security → Open Anyway** if you trust the download.

Syntax and project formats may change. Keep backups of your work. Include `BUILD.json` when [reporting an issue](https://github.com/martineastwood/niminal/issues).

The `.sha256` asset contains the DMG's SHA-256 checksum.
