#!/bin/sh
# Builds rill-stream's browser wasm and DOM module at a revision into a directory (for
# paired browser A/B runs). Usage: scripts/ab-stream-web.sh REV OUT
set -eu
cd "$(dirname "$0")/.."
rev="$1"; out="$2"
tmp=$(mktemp -d)
git worktree add -q --detach "$tmp/tree" "$rev"
ln -s "$PWD/node_modules" "$tmp/tree/node_modules"
(cd "$tmp/tree" && PATH="$HOME/.cargo/bin:$PATH" sh scripts/build-stream-web.sh "$out" && cp js/src/stream-dom.ts "$out/")
git worktree remove --force "$tmp/tree"
rm -rf "$tmp"
