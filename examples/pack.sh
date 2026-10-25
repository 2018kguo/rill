#!/bin/sh
# Packs the JavaScript package (as `npm publish` would) into examples/.pack/rill.tgz,
# which every example installs. Needs the WebAssembly built first
# (scripts/build-wasm.sh), or pass --build to build it.
set -eu
cd "$(dirname "$0")/.."
if [ "${1:-}" = "--build" ]; then sh scripts/build-wasm.sh; fi
mkdir -p examples/.pack
(cd js && npx --no-install tsc -p . && npm pack --silent --ignore-scripts --pack-destination ../examples/.pack >/dev/null)
mv examples/.pack/2018kguo-rill-*.tgz examples/.pack/rill.tgz
