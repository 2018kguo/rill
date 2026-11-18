#!/bin/sh
# Builds rill-stream's WebAssembly for browsers into $1 (default js/stream-web).
set -eu
cd "$(dirname "$0")/.."
out="${1:-js/stream-web}"
RUSTFLAGS="-C target-feature=+simd128,+bulk-memory --remap-path-prefix=$HOME=~ --remap-path-prefix=$PWD=." \
cargo build -q --release -p rill-stream --features wasm --target wasm32-unknown-unknown \
  --config profile.release.lto=true --config profile.release.codegen-units=1 \
  --config profile.release.debug=false --config profile.release.strip=true --config profile.release.panic='"abort"'
wasm-bindgen --target web --out-dir "$out" target/wasm32-unknown-unknown/release/rill_stream.wasm
npx --no-install wasm-opt -O3 "$out/rill_stream_bg.wasm" -o "$out/rill_stream_bg.wasm" \
  --enable-simd --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
  --enable-mutable-globals --enable-multivalue --enable-reference-types
