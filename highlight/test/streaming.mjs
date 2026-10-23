// Streaming differential test: rill-highlight's code plugin (wasm) vs
// @streamdown/code's, on every streamed prefix of corpus files (first MAX_LINES
// lines of each), per language.   node test/streaming.mjs [LANG...]
import { execFileSync } from 'node:child_process';
import { readFileSync, existsSync } from 'node:fs';

const here = new URL('..', import.meta.url).pathname;
const maxLines = process.env.MAX_LINES ?? '150';
const index = JSON.parse(readFileSync(here + 'corpus/files/index.json', 'utf8'));
const edges = existsSync(here + 'corpus/edge/index.json') ? JSON.parse(readFileSync(here + 'corpus/edge/index.json', 'utf8')) : [];
const entries = [...index.map((e) => ({ lang: e.lang, path: here + 'corpus/files/' + e.file })), ...edges.map((e) => ({ lang: e.lang, path: here + 'corpus/edge/' + e.file }))];
const wanted = process.argv.slice(2);
const langs = [...new Set(entries.map((e) => e.lang))].filter((l) => !wanted.length || wanted.includes(l));
const run = (engine, lang, files) => execFileSync('node', [here + 'test/stream-run.mjs', lang, maxLines, ...files], { env: { ...process.env, ENGINE: engine }, maxBuffer: 2 ** 31 }).toString().trim().split('\n');
const summary = [];
let exact = true;
for (const lang of langs) {
  const files = entries.filter((e) => e.lang === lang).map((e) => e.path);
  let calls = 0, bad = 0;
  // One reference process per file (see compare.mjs on V8's tier-up bug).
  for (const file of files) {
    const want = run('shiki', lang, [file]);
    const got = run('rill', lang, [file]);
    for (let i = 0; i < Math.max(want.length, got.length); i++) {
      calls++;
      if (want[i] !== got[i]) {
        bad++;
        if (bad <= 2) console.log(`${lang} ${file.split('/').pop()} call ${i}:\n  want ${want[i]?.slice(0, 400)}\n  got  ${got[i]?.slice(0, 400)}`);
      }
    }
  }
  if (bad) exact = false;
  summary.push({ lang, files: files.length, calls, mismatches: bad });
  console.log(JSON.stringify(summary.at(-1)));
}
console.table(summary);
process.exit(exact ? 0 : 1);
