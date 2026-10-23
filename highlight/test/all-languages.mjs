// Every bundled language on a mixed sample (the first lines of several corpus
// files, so each grammar sees code it was and was not written for): Rust tokens
// vs @streamdown/code's.   node test/all-languages.mjs [LANG...]
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { bundledLanguagesInfo } from 'shiki';

const here = new URL('..', import.meta.url).pathname;
const index = JSON.parse(readFileSync(here + 'corpus/files/index.json', 'utf8'));
const sampleLangs = ['typescript', 'python', 'c', 'html', 'markdown', 'shellscript', 'ruby', 'json', 'yaml', 'sql'];
const sample = sampleLangs.map((l) => readFileSync(here + 'corpus/files/' + index.find((e) => e.lang === l).file, 'utf8').split('\n').slice(0, 40).join('\n')).join('\n');
const samplePath = here + 'scratch/mixed-sample.txt';
writeFileSync(samplePath, sample);
const wanted = process.argv.slice(2);
const ids = bundledLanguagesInfo.map((i) => i.id).filter((id) => !wanted.length || wanted.includes(id));
const results = { exact: [], mismatch: [], error: [] };
for (const id of ids) {
  let rust, ref;
  try {
    rust = JSON.parse(execFileSync(here + '../target/release/examples/tokens', [here + 'dist', id, samplePath], { maxBuffer: 1 << 30 }).toString().trim().split('\n')[0]);
    ref = JSON.parse(execFileSync('node', [here + 'test/reference.mjs', id, samplePath], { maxBuffer: 1 << 30 }).toString());
  } catch (e) {
    results.error.push(`${id}: ${String(e).slice(0, 200)}`);
    continue;
  }
  if (rust.error) {
    results.error.push(`${id}: ${rust.error.slice(0, 200)}`);
    continue;
  }
  const bad = ref.tokens.findIndex((line, i) => JSON.stringify(line) !== JSON.stringify(rust.tokens[i]));
  if (bad === -1 && ref.tokens.length === rust.tokens.length) results.exact.push(id);
  else results.mismatch.push(`${id} line ${bad + 1}: want ${JSON.stringify(ref.tokens[bad])?.slice(0, 300)} got ${JSON.stringify(rust.tokens[bad])?.slice(0, 300)}`);
  process.stderr.write(bad === -1 ? '.' : 'x');
}
console.log(`\nexact ${results.exact.length}/${ids.length}`);
console.log('MISMATCH\n' + results.mismatch.join('\n'));
console.log('ERROR\n' + results.error.join('\n'));
