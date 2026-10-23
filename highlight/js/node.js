// Node helper: a core reading the wasm and data from a dist directory.
import { readFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createHighlightCore } from './core.js';

export function createNodeHighlightCore(dist = new URL('../dist/', import.meta.url)) {
  const base = dist instanceof URL ? dist : new URL(`file://${dist.endsWith('/') ? dist : dist + '/'}`);
  return createHighlightCore({
    wasm: readFileSync(new URL('rill_highlight.wasm', base)),
    data: {
      languages: JSON.parse(readFileSync(new URL('languages.json', base), 'utf8')),
      grammar: (name) => readFile(new URL(`grammars/${name}.json`, base), 'utf8'),
      theme: (name) => readFile(new URL(`themes/${name}.json`, base), 'utf8'),
    },
  });
}
