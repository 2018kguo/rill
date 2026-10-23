// Differential test: Rust tokens (examples/tokens) vs @streamdown/code's, per corpus
// language, both themes, exact JSON (token boundaries, offsets, htmlStyle key order).
//   node test/compare.mjs [LANG...]
// The reference runs one Node process per file: V8's RegExp tier-up can return
// wrong results for case-insensitive modifier groups after enough work in one
// process, which would make the reference itself depend on what ran before.
import { execFileSync } from 'node:child_process';
import { readFileSync, existsSync } from 'node:fs';

const here = new URL('..', import.meta.url).pathname;
const corpus = here + 'corpus/files/';
const index = JSON.parse(readFileSync(corpus + 'index.json', 'utf8'));
const edges = existsSync(here + 'corpus/edge/index.json') ? JSON.parse(readFileSync(here + 'corpus/edge/index.json', 'utf8')) : [];
const entries = [...index.map((e) => ({ lang: e.lang, path: corpus + e.file })), ...edges.map((e) => ({ lang: e.lang, path: here + 'corpus/edge/' + e.file }))];
const wanted = process.argv.slice(2);
const langs = [...new Set(entries.map((e) => e.lang))].filter((l) => !wanted.length || wanted.includes(l));
const rustBin = here + '../target/release/examples/tokens';
const nodeFlags = process.env.REF_FLAGS ? process.env.REF_FLAGS.split(' ') : [];
let allExact = true;
const summary = [];
for (const lang of langs) {
  const files = entries.filter((e) => e.lang === lang).map((e) => e.path);
  const rust = execFileSync(rustBin, [here + 'dist', lang, ...files], { maxBuffer: 1 << 30 }).toString().trim().split('\n').map((l) => JSON.parse(l));
  let lines = 0, tokens = 0, badLines = 0, shown = 0;
  for (const [i, file] of files.entries()) {
    const ref = JSON.parse(execFileSync('node', [...nodeFlags, here + 'test/reference.mjs', lang, file], { maxBuffer: 1 << 30 }).toString());
    const got = rust[i] ?? rust[0];
    if (got.error) { console.log(`${lang} ${file}: rust error ${got.error}`); badLines += ref.tokens.length; continue; }
    for (const key of ['fg', 'bg', 'themeName']) if (ref[key] !== got[key]) { console.log(`${lang}: ${key} ${got[key]} != ${ref[key]}`); badLines++; }
    for (let l = 0; l < Math.max(ref.tokens.length, got.tokens.length); l++) {
      lines++;
      tokens += ref.tokens[l]?.length ?? 0;
      const a = JSON.stringify(ref.tokens[l]), b = JSON.stringify(got.tokens[l]);
      if (a !== b) {
        badLines++;
        if (shown++ < 3) console.log(`${lang} ${file.split('/').pop()}:${l + 1}\n  want ${a?.slice(0, 600)}\n  got  ${b?.slice(0, 600)}`);
      }
    }
  }
  if (badLines) allExact = false;
  summary.push({ lang, files: files.length, lines, tokens, mismatchedLines: badLines });
  console.log(JSON.stringify(summary.at(-1)));
}
console.table(summary);
process.exit(allExact ? 0 : 1);
