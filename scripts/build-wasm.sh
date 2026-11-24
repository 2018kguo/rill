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
# rill-stream, the streaming renderer for Streamdown's default setup: its own module,
# built for size (its work is a small share of an update's) with std rebuilt to trap on
# a panic instead of formatting one, which needs a pinned nightly:
#   rustup toolchain install nightly-2026-11-23 --profile minimal --component rust-src --target wasm32-unknown-unknown
# SIMD128 is in every current browser engine; memchr's scans use it.
RUSTFLAGS="$RUSTFLAGS -C target-feature=+simd128,+bulk-memory" \
cargo +nightly-2026-11-23 build --release -p rill-stream --features wasm --target wasm32-unknown-unknown \
  -Zpanic-immediate-abort -Zbuild-std=std,panic_abort -Zbuild-std-features=optimize_for_size \
  --config profile.release.lto=true --config profile.release.codegen-units=1 \
  --config profile.release.debug=false --config profile.release.strip=true \
  --config profile.release.panic='"immediate-abort"' --config profile.release.opt-level='"z"'
"${WASM_BINDGEN:-wasm-bindgen}" target/wasm32-unknown-unknown/release/rill_stream.wasm \
  --target web --out-dir js/wasm --out-name rill_stream
npx --no-install wasm-opt -Oz js/wasm/rill_stream_bg.wasm -o js/wasm/rill_stream_bg.wasm \
  --enable-simd --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
  --enable-mutable-globals --enable-multivalue --enable-reference-types
