#!/bin/sh
# Check, then install every program for dogfooding (default release profile, not release-fast).
# `niminal` and `niminal-live` each carry their own copy of the language, so installing one leaves the other stale.
# Pass `--test` to run the test suites in the check as well.
set -e
cd "$(dirname "$0")/.."
scripts/check.sh "$@"
cargo install --path crates/niminal-cli
cargo install --path sequencer/live-cli
echo "installed niminal and niminal-live; restart any running niminal server or niminal-live (they keep the old build until you do)"
