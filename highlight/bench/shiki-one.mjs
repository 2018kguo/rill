// Times @streamdown/code's Shiki setup on one language (fresh process): cold =
// createHighlighter + first codeToTokens; warm = codeToTokens of the same code.
import { readFileSync } from 'node:fs';
import { createHighlighter } from 'shiki';
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript';
const [lang, ...files] = process.argv.slice(2);
const code = files.map((f) => readFileSync(f, 'utf8')).join('\n');
const t0 = performance.now();
const highlighter = await createHighlighter({ themes: ['github-light', 'github-dark'], langs: [lang], engine: createJavaScriptRegexEngine({ forgiving: true }) });
const options = { lang, themes: { light: 'github-light', dark: 'github-dark' } };
highlighter.codeToTokens(code, options);
const coldMs = performance.now() - t0;
let runs = 0;
const t1 = performance.now();
while (performance.now() - t1 < 1000 || runs < 3) { highlighter.codeToTokens(code, options); runs++; }
console.log(JSON.stringify({ lang, coldMs, warmMs: (performance.now() - t1) / runs }));
