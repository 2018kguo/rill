#!/bin/sh
# Builds the example's WebAssembly into examples/custom-build/pkg, as a build of your own
# would: cargo for wasm32, then `wasm-bindgen --target web` (0.2.129, the version rill's
# crate pins).
set -eu
cd "$(dirname "$0")"
cargo build --release -p rill-custom-build --lib --target wasm32-unknown-unknown
"${WASM_BINDGEN:-wasm-bindgen}" ../../target/wasm32-unknown-unknown/release/rill_custom_build.wasm \
  --target web --out-dir pkg --out-name custom
