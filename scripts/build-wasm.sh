#!/bin/sh
# Builds the prebuilt WebAssembly the JavaScript package ships into js/wasm. Needs
# `rustup target add wasm32-unknown-unknown` and
# `cargo install wasm-bindgen-cli --version 0.2.129`.
set -eu
cd "$(dirname "$0")/.."
cargo build --release -p rill --lib --features wasm-exports --target wasm32-unknown-unknown \
  --config profile.release.lto=true --config profile.release.codegen-units=1 --config profile.release.debug=false --config profile.release.strip=true
"${WASM_BINDGEN:-wasm-bindgen}" target/wasm32-unknown-unknown/release/rill.wasm \
  --target web --out-dir js/wasm --out-name rill
