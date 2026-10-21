#!/bin/sh
# Runs what CI runs, so a push doesn't find the failures first. Use before pushing.
# Clippy only by default (most CI failures); `--test` adds the test suites.
set -e
cd "$(dirname "$0")/.."
cargo clippy --workspace --all-targets -- -D warnings
(cd sequencer && cargo clippy --all-targets -- -D warnings)
# needs: rustup target add wasm32-unknown-unknown
cargo build -p niminal-engine --target wasm32-unknown-unknown
if [ "$1" = "--test" ]; then
  cargo test --workspace
  (cd sequencer && cargo test)
fi
