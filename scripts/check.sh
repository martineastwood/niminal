#!/bin/sh
# Runs what CI runs, so a push doesn't find the failures first.
#   scripts/check.sh                  clippy on the whole Rust workspace (what most CI failures are)
#   scripts/check.sh --test           ...and every Rust test
#   scripts/check.sh --apps           ...and Stage (clippy, cargo test, tsc, vitest) and the extension's tests
# The options combine: `scripts/check.sh --test --apps` is everything.
set -e
cd "$(dirname "$0")/.."
test=0
apps=0
for arg in "$@"; do
  case "$arg" in
    --test) test=1 ;;
    --apps) apps=1 ;;
    *) echo "unknown option $arg" >&2; exit 2 ;;
  esac
done

cargo clippy --workspace --all-targets -- -D warnings
if [ $test = 1 ]; then cargo test --workspace; fi

if [ $apps = 1 ]; then
  (cd stage/src-tauri && cargo clippy --all-targets -- -D warnings)
  if [ $test = 1 ]; then (cd stage/src-tauri && cargo test); fi
  (cd stage && npx tsc --noEmit)
  if [ $test = 1 ]; then (cd stage && npx vitest run); fi
  if [ $test = 1 ]; then (cd editors/vscode && npm test); fi
fi
