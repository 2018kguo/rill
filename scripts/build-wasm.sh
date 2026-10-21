#!/bin/sh
# Builds rill for the browser into demo/pkg. Needs `rustup target add wasm32-unknown-unknown`
# and `cargo install wasm-bindgen-cli --version 0.2.129`.
set -eu
cd "$(dirname "$0")/.."
cargo build --release -p rill --lib --target wasm32-unknown-unknown \
  --config profile.release.lto=true --config profile.release.codegen-units=1 --config profile.release.debug=false
"${WASM_BINDGEN:-wasm-bindgen}" target/wasm32-unknown-unknown/release/rill.wasm \
  --target web --out-dir demo/pkg --out-name rill
