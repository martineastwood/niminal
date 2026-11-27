#!/bin/sh
# Builds what the web pages load: the engine as WebAssembly, its loader and the examples.
# Needs the wasm32 target: `rustup target add wasm32-unknown-unknown`.
set -e
cd "$(dirname "$0")/../.."
# the repository builds for the machine it is on (.cargo/config.toml); WebAssembly has no such CPU
RUSTFLAGS="" cargo build -p niminal-web --profile web --target wasm32-unknown-unknown
out=docs/public/web
mkdir -p "$out/examples"
cp target/wasm32-unknown-unknown/web/niminal_web.wasm "$out/"
cp crates/niminal-web/js/niminal.js "$out/"
# examples that need no sample files
for name in functions kernels acid physical_models modal array_bank; do
  cp "examples/$name.nml" "$out/examples/"
done
# the live sets of the Studio
mkdir -p "$out/studio"
cp docs/studio/*.nml "$out/studio/"
# the pieces of the "Try it" page, and what its sliders move
mkdir -p "$out/try"
cp docs/try/*.nml docs/try/manifest.json "$out/try/"
# pieces in the list that are also repository examples are used as they are, so there is one copy of each
for name in $(sed -n 's/.*"file": "\([a-z_]*\)".*/\1/p' docs/try/manifest.json); do
  [ -f "$out/try/$name.nml" ] || cp "examples/$name.nml" "$out/try/"
done
echo "web: $(wc -c < "$out/niminal_web.wasm" | tr -d ' ') bytes of wasm, $(ls "$out/examples" | wc -l | tr -d ' ') examples"
