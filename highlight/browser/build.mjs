// Bundles the bench entries into dist/browser and writes the answer blocks.
import { execFileSync } from 'node:child_process';
const here = new URL('..', import.meta.url).pathname;
execFileSync('bun', ['build', here + 'browser/shiki-entry.js', here + 'browser/rill-entry.js', '--outdir', here + 'dist/browser', '--target', 'browser', '--splitting', '--minify', '--format', 'esm'], { stdio: 'inherit' });
execFileSync('node', [here + 'browser/answers.mjs'], { stdio: 'inherit' });
