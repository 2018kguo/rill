// Shiki (Node/V8) vs rill-highlight (wasm in Node/V8) on the corpus, per language.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
const here = new URL('..', import.meta.url).pathname;
const index = JSON.parse(readFileSync(here + 'corpus/files/index.json', 'utf8'));
const wanted = process.argv.slice(2);
const langs = [...new Set(index.map((e) => e.lang))].filter((l) => !wanted.length || wanted.includes(l));
const rows = [];
for (const lang of langs) {
  const files = index.filter((e) => e.lang === lang).map((e) => here + 'corpus/files/' + e.file);
  const js = JSON.parse(execFileSync('node', [here + 'bench/shiki-one.mjs', lang, ...files]).toString());
  const rs = JSON.parse(execFileSync('node', [here + 'bench/rill-one.mjs', lang, ...files]).toString());
  rows.push({ lang, shikiColdMs: +js.coldMs.toFixed(1), rillColdMs: +rs.coldMs.toFixed(1), shikiWarmMs: +js.warmMs.toFixed(2), rillWarmMs: +rs.warmMs.toFixed(2), warmSpeedup: +(js.warmMs / rs.warmMs).toFixed(1), coldSpeedup: +(js.coldMs / rs.coldMs).toFixed(1) });
  console.error(JSON.stringify(rows.at(-1)));
}
console.table(rows);
const sum = (k) => rows.reduce((a, r) => a + r[k], 0);
console.log(`total: Shiki warm ${sum('shikiWarmMs').toFixed(0)} ms, rill wasm warm ${sum('rillWarmMs').toFixed(0)} ms (${(sum('shikiWarmMs') / sum('rillWarmMs')).toFixed(1)}x); cold ${sum('shikiColdMs').toFixed(0)} vs ${sum('rillColdMs').toFixed(0)} ms (${(sum('shikiColdMs') / sum('rillColdMs')).toFixed(1)}x)`);
