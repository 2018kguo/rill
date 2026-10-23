// Builds dist/bench/answers.json: code blocks the size of AI answers (60 lines)
// cut from the corpus, in the languages answers use most.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
const here = new URL('..', import.meta.url).pathname;
const index = JSON.parse(readFileSync(here + 'corpus/files/index.json', 'utf8'));
const langs = ['typescript', 'tsx', 'python', 'javascript', 'shellscript', 'json', 'go', 'rust', 'java', 'cpp', 'sql', 'yaml', 'css', 'html', 'csharp', 'markdown'];
const blocks = langs.map((lang) => {
  const entry = index.find((e) => e.lang === lang);
  const lines = readFileSync(here + 'corpus/files/' + entry.file, 'utf8').split('\n');
  const start = Math.min(40, Math.max(0, lines.length - 60));
  return { lang, code: lines.slice(start, start + 60).join('\n') + '\n' };
});
mkdirSync(here + 'dist/bench', { recursive: true });
writeFileSync(here + 'dist/bench/answers.json', JSON.stringify(blocks));
console.log(blocks.length, 'blocks,', blocks.reduce((n, b) => n + b.code.length, 0), 'chars');
