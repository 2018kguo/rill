// Reference tokens: @streamdown/code 2.0.0's `highlight()` (Shiki on its JavaScript
// regex engine), one JSON line per file, shaped like the Rust `tokens` example.
//   node test/reference.mjs LANG FILE...
import { readFileSync } from 'node:fs';
import { createCodePlugin } from '@streamdown/code';

const [lang, ...files] = process.argv.slice(2);
const plugin = createCodePlugin();
const highlight = (code) =>
  new Promise((resolve) => {
    const cached = plugin.highlight({ code, language: lang, themes: ['github-light', 'github-dark'] }, resolve);
    if (cached) resolve(cached);
  });
for (const file of files) {
  const code = readFileSync(file, 'utf8');
  const result = await highlight(code);
  const tokens = result.tokens.map((line) => line.map((t) => [t.content, t.offset, t.htmlStyle]));
  console.log(JSON.stringify({ file, fg: result.fg, bg: result.bg, themeName: result.themeName, tokens }));
}
