#!/bin/sh
# Builds rill-stream's WebAssembly for the Node tests (js/stream-wasm).
set -eu
cd "$(dirname "$0")/.."
cargo build -q --release -p rill-stream --features wasm --target wasm32-unknown-unknown
wasm-bindgen --target nodejs --out-dir js/stream-wasm target/wasm32-unknown-unknown/release/rill_stream.wasm
echo '{"type":"commonjs"}' > js/stream-wasm/package.json
