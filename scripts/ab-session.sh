#!/bin/sh
# Paired A/B of the session example: HEAD's crates/rill-stream vs the working tree.
# Usage: scripts/ab-session.sh FILE [ROUNDS]
set -eu
cd "$(dirname "$0")/.."
file="$1"; rounds="${2:-6}"
tmp=$(mktemp -d)
git worktree add -q --detach "$tmp/head" HEAD
(cd "$tmp/head" && CARGO_TARGET_DIR="$tmp/target" cargo build -q --release -p rill-stream --example session)
cargo build -q --release -p rill-stream --example session
for round in $(seq "$rounds"); do
  a=$("$tmp/target/release/examples/session" "$file" | grep '^total' | awk '{print $2}')
  b=$(./target/release/examples/session "$file" | grep '^total' | awk '{print $2}')
  echo "round $round: HEAD $a µs, tree $b µs"
done
git worktree remove --force "$tmp/head"
rm -rf "$tmp"
