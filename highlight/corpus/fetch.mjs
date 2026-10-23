// Downloads the corpus listed in sources.json (pinned commits) into corpus/files/.
// Files are capped at `maxLines` lines. `@commit` entries fetch that commit's diff.
import { mkdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join } from 'node:path';

const here = dirname(new URL(import.meta.url).pathname);
const { repos, files, maxLines } = JSON.parse(readFileSync(join(here, 'sources.json'), 'utf8'));
const outDir = join(here, 'files');
mkdirSync(outDir, { recursive: true });

const entries = [];
for (const [lang, repoKey, path] of files) {
  const [repo, sha, license] = repos[repoKey];
  const name = `${lang}--${repoKey}--${path.replace(/[^A-Za-z0-9._-]+/g, '_')}`;
  const target = join(outDir, name);
  if (!existsSync(target)) {
    const url = path === '@commit'
      ? `https://github.com/${repo}/commit/${sha}.diff`
      : `https://raw.githubusercontent.com/${repo}/${sha}/${path}`;
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${url}: ${response.status}`);
    const text = (await response.text()).split('\n').slice(0, maxLines).join('\n');
    writeFileSync(target, text);
  }
  entries.push({ lang, file: name, repo, sha, path, license });
}
writeFileSync(join(outDir, 'index.json'), JSON.stringify(entries, null, 1));
console.log(`${entries.length} files in ${outDir}`);
