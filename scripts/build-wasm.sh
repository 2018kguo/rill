#!/bin/sh
# Builds the prebuilt WebAssembly the JavaScript package ships into js/wasm. Needs
# `rustup target add wasm32-unknown-unknown`, `cargo install wasm-bindgen-cli --version
# 0.2.129`, and `npm install` (for binaryen's wasm-opt).
set -eu
cd "$(dirname "$0")/.."
# Panic messages embed source paths; keep the build machine's out of the shipped file.
export RUSTFLAGS="--remap-path-prefix=$HOME=~ --remap-path-prefix=$PWD=."
cargo build --release -p rill --lib --features wasm-exports --target wasm32-unknown-unknown \
  --config profile.release.lto=true --config profile.release.codegen-units=1 \
  --config profile.release.debug=false --config profile.release.strip=true
"${WASM_BINDGEN:-wasm-bindgen}" target/wasm32-unknown-unknown/release/rill.wasm \
  --target web --out-dir js/wasm --out-name rill
# About 3% faster and 6% smaller. The features are the ones rustc enables by default for
# wasm32-unknown-unknown; wasm-opt rejects a module that uses any other.
npx --no-install wasm-opt -O3 js/wasm/rill_bg.wasm -o js/wasm/rill_bg.wasm \
  --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
  --enable-mutable-globals --enable-multivalue --enable-reference-types
