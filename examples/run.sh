#!/bin/sh
# Builds and serves an example in each mode given, and runs smoke.mjs against it:
#   sh examples/run.sh nextjs dev dev-webpack build build-webpack
#   sh examples/run.sh vite dev build vite7-dev vite7-build
#   sh examples/run.sh react-router dev build
# Installs the example first (with examples/.pack/rill.tgz from pack.sh). The vite7
# modes install Vite 7 over Vite 8, so they go last.
set -eu
cd "$(dirname "$0")"
example=$1
shift
cd "$example"
rm -rf node_modules/@2018kguo/rill
npm install --no-audit --no-fund --no-package-lock --silent
bin=node_modules/.bin
port=${PORT:-4790}
ssr=""
warmup=""

serve() {
  "$@" &
  server=$!
  for _ in $(seq 120); do
    if curl -sf -o /dev/null "http://localhost:$port/"; then break; fi
    sleep 0.5
  done
  status=0
  node ../smoke.mjs "http://localhost:$port/" $ssr $warmup || status=$?
  kill "$server" 2>/dev/null || true
  wait "$server" 2>/dev/null || true
  [ "$status" -eq 0 ] || exit "$status"
}

for mode in "$@"; do
  echo "== $example $mode"
  case "$example:$mode" in
    nextjs:dev) rm -rf .next; serve $bin/next dev -p "$port" ;;
    nextjs:dev-webpack) rm -rf .next; serve $bin/next dev --webpack -p "$port" ;;
    nextjs:build) rm -rf .next; $bin/next build; serve $bin/next start -p "$port" ;;
    nextjs:build-webpack) rm -rf .next; $bin/next build --webpack; serve $bin/next start -p "$port" ;;
    vite:dev) ssr=--no-ssr; serve $bin/vite --port "$port" --strictPort ;;
    vite:build) ssr=--no-ssr; $bin/vite build; serve $bin/vite preview --port "$port" --strictPort ;;
    vite:vite7-*)
      ssr=--no-ssr
      npm install --no-audit --no-fund --no-package-lock --silent --no-save vite@7.3.1 @vitejs/plugin-react@5.1.2
      rm -rf node_modules/.vite
      if [ "$mode" = vite7-dev ]; then
        serve $bin/vite --config vite7.config.ts --port "$port" --strictPort
      else
        $bin/vite build --config vite7.config.ts
        serve $bin/vite preview --config vite7.config.ts --port "$port" --strictPort
      fi
      ;;
    # Vite discovers React Router's client dependencies on the first page load and
    # reloads it once; the smoke test loads the page once before it starts.
    react-router:dev) warmup=--warmup; serve $bin/react-router dev --port "$port" --strictPort ;;
    react-router:build) warmup=""; $bin/react-router build; PORT=$port serve $bin/react-router-serve ./build/server/index.js ;;
    *) echo "unknown mode: $example $mode" >&2; exit 2 ;;
  esac
done
