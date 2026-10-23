#!/bin/sh
# Packs @2018kguo/rill-highlight from a checkout (default ../rill-highlight, built with
# its `npm run build`) into vendor/, where js/package.json's `file:` dependency points
# until the two packages are published together. Then `npm install`.
set -eu
cd "$(dirname "$0")/.."
source="$(cd "${1:-../rill-highlight}/js" && pwd)"
mkdir -p vendor
vendor="$(pwd)/vendor"
(cd "$source" && npm pack --ignore-scripts --pack-destination "$vendor")
