#!/bin/sh
# Runs what CI runs, so a push doesn't find the failures first. Use before pushing.
# Clippy only by default (most CI failures); `--test` adds the test suites.
# The engine's workspace and the sequencer's have separate build directories, so they run side by side.
set -e
cd "$(dirname "$0")/.."
log=$(mktemp -d)
trap 'rm -rf "$log"' EXIT

engine() {
  cargo clippy --workspace --all-targets -- -D warnings
  # needs: rustup target add wasm32-unknown-unknown
  cargo build -p niminal-engine --target wasm32-unknown-unknown
  if [ "$1" = "--test" ]; then cargo test --workspace; fi
}
sequencer() {
  cd sequencer
  cargo clippy --all-targets -- -D warnings
  if [ "$1" = "--test" ]; then cargo test; fi
}

(engine "$@") >"$log/engine" 2>&1 & engine_pid=$!
(sequencer "$@") >"$log/sequencer" 2>&1 & sequencer_pid=$!
failed=0
wait $engine_pid || failed=1
echo "== engine workspace"; cat "$log/engine"
wait $sequencer_pid || failed=1
echo "== sequencer workspace"; cat "$log/sequencer"
exit $failed
