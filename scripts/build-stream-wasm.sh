#!/bin/sh
# Builds rill-stream's WebAssembly for the Node tests (js/stream-wasm). SIMD128 is in every
# current browser engine; memchr's scans use it.
set -eu
cd "$(dirname "$0")/.."
RUSTFLAGS="-C target-feature=+simd128,+bulk-memory --remap-path-prefix=$HOME=~ --remap-path-prefix=$PWD=." \
cargo build -q --release -p rill-stream --features wasm --target wasm32-unknown-unknown \
  --config profile.release.lto=true --config profile.release.codegen-units=1 \
  --config profile.release.debug=false --config profile.release.strip=true --config profile.release.panic='"abort"'
wasm-bindgen --target nodejs --out-dir js/stream-wasm target/wasm32-unknown-unknown/release/rill_stream.wasm
npx --no-install wasm-opt -O3 js/stream-wasm/rill_stream_bg.wasm -o js/stream-wasm/rill_stream_bg.wasm \
  --enable-simd --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
  --enable-mutable-globals --enable-multivalue --enable-reference-types
echo '{"type":"commonjs"}' > js/stream-wasm/package.json
