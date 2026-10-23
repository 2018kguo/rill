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
// The minimum of several runs after warming up: the machine may be shared.
for (let i = 0; i < 3; i++) { highlighter.codeToTokens(code); }
let warmMs = Infinity;
for (let i = 0; i < 9; i++) { const t = performance.now(); highlighter.codeToTokens(code); warmMs = Math.min(warmMs, performance.now() - t); }
console.log(JSON.stringify({ lang, coldMs, warmMs }));
