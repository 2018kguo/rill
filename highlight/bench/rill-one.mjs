// Times rill-highlight (wasm in Node) on one language (fresh process), like
// shiki-one.mjs: cold = getHighlighter + first codeToTokens; warm = codeToTokens.
import { readFileSync } from 'node:fs';
import { createNodeHighlightCore } from '../js/node.js';
const [lang, ...files] = process.argv.slice(2);
const code = files.map((f) => readFileSync(f, 'utf8')).join('\n');
const core = await createNodeHighlightCore();
const t0 = performance.now();
const highlighter = await core.getHighlighter(lang, ['github-light', 'github-dark']);
highlighter.codeToTokens(code);
const coldMs = performance.now() - t0;
let runs = 0;
const t1 = performance.now();
while (performance.now() - t1 < 1000 || runs < 3) { highlighter.codeToTokens(code); runs++; }
console.log(JSON.stringify({ lang, coldMs, warmMs: (performance.now() - t1) / runs }));
