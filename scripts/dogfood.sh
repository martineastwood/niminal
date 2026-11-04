#!/bin/sh
# Check, then install the niminal command for dogfooding (default release profile, not release-fast).
# Stage carries its own copy of the language, so rebuild it too: `cd stage && npm run tauri -- build --bundles app`.
# Pass `--test` to run the test suites in the check as well.
set -e
cd "$(dirname "$0")/.."
scripts/check.sh "$@"
# The checks above have already downloaded the locked dependencies. Reuse them
# instead of resolving newer versions and updating the index on every install.
# A build directory that stays between runs, so only what changed is built again (the default is a fresh one each time).
cargo install --locked --offline --target-dir sequencer/target/install --path sequencer/live-cli
echo "installed niminal; restart any running niminal server or niminal play (they keep the old build until you do)"
