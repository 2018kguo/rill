#!/bin/sh
# Two production builds of Vercel's ai-chatbot template, one per engine, on one local
# Postgres database:
#   sh setup.sh <dir> <postgres-url>      e.g. sh setup.sh ../../../ai-chatbot postgres://localhost/aichatbot_bench
# <dir>/streamdown is the template with template.patch (Streamdown 2.7.1, the benchmark's
# mock answer, plain cookies over http://localhost, no BotID); <dir>/rill is the same
# plus rill.patch (the import, in the two files that render Markdown) and the package
# from examples/.pack/rill.tgz (examples/pack.sh). Needs git, node and psql.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
dir=$1
url=$2
commit=c2f8235e1f3ea903ad8b7f61447c4f74164b5c58   # vercel/ai-chatbot main, Jul 8 2026
pnpm="npx -y pnpm@10.32.1"

git clone -q https://github.com/vercel/ai-chatbot "$dir/streamdown"
cd "$dir/streamdown"
git checkout -q "$commit"
git apply "$here/template.patch"
node -e 'const fs = require("fs"); fs.writeFileSync("lib/ai/bench-answer.ts", "// Generated from rill bench/ai-chatbot/answer.md.\nexport const BENCH_ANSWER = " + JSON.stringify(fs.readFileSync(process.argv[1], "utf8")) + ";\n")' "$here/answer.md"
printf 'AUTH_SECRET=%s\nPOSTGRES_URL=%s\nPLAYWRIGHT=True\n' "$(openssl rand -base64 32)" "$url" > .env.local
$pnpm install
npx tsx lib/db/migrate.ts
node "$here/seed.mjs" | psql -q "$url"

cp -R "$dir/streamdown" "$dir/rill"
cd "$dir/rill"
git apply "$here/rill.patch"
cp "$here/../../examples/.pack/rill.tgz" ../rill.tgz
$pnpm add ../rill.tgz

for engine in streamdown rill; do (cd "$dir/$engine" && npx next build); done
echo "Serve them: (cd $dir/streamdown && npx next start -p 4801) and (cd $dir/rill && npx next start -p 4802)"
