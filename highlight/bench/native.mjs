// Shiki (Node/V8) vs rill-highlight (native) on the corpus, per language.
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
const here = new URL('..', import.meta.url).pathname;
const index = JSON.parse(readFileSync(here + 'corpus/files/index.json', 'utf8'));
const wanted = process.argv.slice(2);
const langs = [...new Set(index.map((e) => e.lang))].filter((l) => !wanted.length || wanted.includes(l));
const rows = [];
for (const lang of langs) {
  const files = index.filter((e) => e.lang === lang).map((e) => here + 'corpus/files/' + e.file);
  const kb = files.reduce((n, f) => n + readFileSync(f).length, 0) / 1024;
  const js = JSON.parse(execFileSync('node', [here + 'bench/shiki-one.mjs', lang, ...files]).toString());
  const rs = JSON.parse(execFileSync(here + '../target/release/examples/bench', [here + 'dist', lang, ...files]).toString());
  rows.push({ lang, kb: +kb.toFixed(1), shikiColdMs: +js.coldMs.toFixed(1), rustColdMs: +rs.coldMs.toFixed(1), shikiWarmMs: +js.warmMs.toFixed(2), rustWarmMs: +rs.warmMs.toFixed(2), warmSpeedup: +(js.warmMs / rs.warmMs).toFixed(1), coldSpeedup: +(js.coldMs / rs.coldMs).toFixed(1) });
  console.error(JSON.stringify(rows.at(-1)));
}
console.table(rows);
const sum = (k) => rows.reduce((a, r) => a + r[k], 0);
console.log(`total: Shiki warm ${sum('shikiWarmMs').toFixed(0)} ms, Rust warm ${sum('rustWarmMs').toFixed(0)} ms (${(sum('shikiWarmMs') / sum('rustWarmMs')).toFixed(1)}x); cold ${sum('shikiColdMs').toFixed(0)} vs ${sum('rustColdMs').toFixed(0)} ms (${(sum('shikiColdMs') / sum('rustColdMs')).toFixed(1)}x)`);
