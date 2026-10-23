#!/bin/sh
# Builds dist/rill_highlight.wasm (raw C-ABI exports, no wasm-bindgen). Needs
# `rustup target add wasm32-unknown-unknown` and the repo's `npm install` (binaryen).
set -eu
cd "$(dirname "$0")/../.."
export RUSTFLAGS="--remap-path-prefix=$HOME=~ --remap-path-prefix=$PWD=."
cargo build --release -p rill-highlight --lib --features wasm-exports --target wasm32-unknown-unknown \
  --config profile.release.lto=true --config profile.release.codegen-units=1 \
  --config profile.release.debug=false --config profile.release.strip=true --config profile.release.panic=\"abort\"
mkdir -p highlight/dist
npx --no-install wasm-opt -O3 target/wasm32-unknown-unknown/release/rill_highlight.wasm -o highlight/dist/rill_highlight.wasm \
  --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
  --enable-mutable-globals --enable-multivalue --enable-reference-types
ls -l highlight/dist/rill_highlight.wasm
