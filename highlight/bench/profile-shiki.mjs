// Step 0: where does Shiki's time go? For each corpus language, measures
//   cold:    createHighlighter + first codeToTokens (includes Oniguruma -> JS translation)
//   total:   warm codeToTokens with dual themes (what @streamdown/code calls)
//   grammar: warm vscode-textmate tokenizeLine2 loop alone (no Shiki theme post-processing)
//   regex:   replay of exactly the RegExp execs the grammar run made
// and records the exec trace (pattern, flags, options, line, start) for the Rust benches.
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { createHighlighter } from 'shiki';
import { createJavaScriptRegexEngine, defaultJavaScriptRegexConstructor } from 'shiki/engine/javascript';
import { toRegExpDetails } from 'oniguruma-to-es';

const corpus = new URL('../corpus/files/', import.meta.url).pathname;
const index = JSON.parse(readFileSync(corpus + 'index.json', 'utf8'));
const onlyLangs = process.argv.slice(2);
const langs = [...new Set(index.map((e) => e.lang))].filter((l) => !onlyLangs.length || onlyLangs.includes(l));
const traceDir = new URL('../scratch/traces/', import.meta.url).pathname;
mkdirSync(traceDir, { recursive: true });

let recording = null; // array while recording execs
const sources = new Map(); // RegExp -> onig source
function regexConstructor(pattern) {
  const regex = defaultJavaScriptRegexConstructor(pattern, { target: 'ES2025' });
  sources.set(regex, pattern);
  const exec = regex.exec;
  regex.exec = function (str) {
    if (recording) recording.push([regex, str, this.lastIndex]);
    return exec.call(this, str);
  };
  return regex;
}

const time = (fn, minMs = 300) => {
  let n = 0; const start = performance.now(); let now = start;
  do { fn(); n++; now = performance.now(); } while (now - start < minMs);
  return (now - start) / n;
};

const rows = [];
for (const lang of langs) {
  const code = index.filter((e) => e.lang === lang).map((e) => readFileSync(corpus + e.file, 'utf8')).join('\n');
  const t0 = performance.now();
  const engine = createJavaScriptRegexEngine({ forgiving: true, regexConstructor });
  const highlighter = await createHighlighter({ themes: ['github-light', 'github-dark'], langs: [lang], engine });
  const opts = { lang, themes: { light: 'github-light', dark: 'github-dark' } };
  highlighter.codeToTokens(code, opts);
  const cold = performance.now() - t0;

  const total = time(() => highlighter.codeToTokens(code, opts));
  const grammar = highlighter.getLanguage(lang);
  const lines = code.split(/\r\n|\r|\n/);
  const runGrammar = () => {
    let state = null;
    for (const line of lines) state = grammar.tokenizeLine2(line, state, 0).ruleStack;
  };
  const grammarMs = time(runGrammar);
  recording = [];
  runGrammar();
  const trace = recording; recording = null;
  const regexMs = time(() => {
    for (const [regex, str, start] of trace) { regex.lastIndex = start; RegExp.prototype.exec.call(regex, str); }
  });
  // Trace for the Rust benches: distinct regexes + (regex id, line id, start).
  const regexIds = new Map(); const lineIds = new Map(); const calls = [];
  const regexList = []; const lineList = []; const results = [];
  for (const [regex, str, start] of trace) {
    if (!regexIds.has(regex)) { regexIds.set(regex, regexList.length); const d = toRegExpDetails(sources.get(regex), { global: true, hasIndices: true, lazyCompileLength: 3000, target: 'ES2025', rules: { allowOrphanBackrefs: true, asciiWordBoundaries: true, captureGroup: true, recursionLimit: 5, singleline: true } }); regexList.push({ onig: sources.get(regex), pattern: d.pattern, flags: d.flags, options: d.options ?? null }); }
    if (!lineIds.has(str)) { lineIds.set(str, lineList.length); lineList.push(str); }
    calls.push(regexIds.get(regex), lineIds.get(str), start);
    regex.lastIndex = start;
    const m = regex.exec(str);
    results.push(m ? m.indices.flatMap((x) => (x ? x : [-1, -1])) : null);
  }
  writeFileSync(traceDir + lang + '.json', JSON.stringify({ regexes: regexList, lines: lineList, calls, results }));
  rows.push({ lang, kb: +(code.length / 1024).toFixed(1), lines: lines.length, coldMs: +cold.toFixed(1), totalMs: +total.toFixed(2), grammarMs: +grammarMs.toFixed(2), regexMs: +regexMs.toFixed(2), execs: trace.length, regexPct: Math.round(100 * regexMs / total), shikiPostPct: Math.round(100 * (total - grammarMs) / total) });
  console.error(JSON.stringify(rows.at(-1)));
}
console.table(rows);
const sum = (k) => rows.reduce((a, r) => a + r[k], 0);
console.log({ totalMs: sum('totalMs').toFixed(1), grammarMs: sum('grammarMs').toFixed(1), regexMs: sum('regexMs').toFixed(1), coldMs: sum('coldMs').toFixed(1), kb: sum('kb').toFixed(0) });
