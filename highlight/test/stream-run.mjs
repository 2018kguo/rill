// Streams each file of one language through a code plugin as Streamdown does:
// growing prefixes with isIncomplete, then the whole file; prints one JSON line per
// call. ENGINE=shiki uses @streamdown/code, ENGINE=rill uses rill-highlight (wasm).
//   ENGINE=rill node test/stream-run.mjs LANG MAX_LINES FILE...
import { readFileSync } from 'node:fs';

const [lang, maxLines, ...files] = process.argv.slice(2);
let plugin;
if (process.env.ENGINE === 'rill') {
  const { createNodeHighlightCore } = await import('../js/node.js');
  const { createCodePlugin } = await import('../js/code-plugin.js');
  plugin = createCodePlugin({ core: await createNodeHighlightCore() });
} else {
  plugin = (await import('@streamdown/code')).createCodePlugin();
}
const themes = ['github-light', 'github-dark'];
const highlight = (code, isIncomplete) =>
  new Promise((resolve) => {
    const cached = plugin.highlight({ code, language: lang, themes, isIncomplete }, resolve);
    if (cached) resolve(cached);
  });
// Deterministic chunk sizes like an LLM stream: 1-24 characters.
let seed = 12345;
const random = () => ((seed = (seed * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
for (const file of files) {
  const code = readFileSync(file, 'utf8').split('\n').slice(0, +maxLines).join('\n');
  let end = 0;
  while (end < code.length) {
    end = Math.min(code.length, end + 1 + Math.floor(random() * 24));
    const result = await highlight(code.slice(0, end), end < code.length);
    const { tokens, fg, bg, themeName, rootStyle } = result;
    console.log(JSON.stringify({ end, fg, bg, themeName, rootStyle, tokens: tokens.map((l) => l.map((t) => [t.content, t.offset, t.htmlStyle, 'explanation' in t])) }));
  }
}
