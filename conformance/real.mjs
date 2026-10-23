// Downloads real-world Markdown from pinned commits into conformance/real/ and writes
// conformance/real.jsonl (one JSON string per document) for diff.mjs and the benches:
//
//   node conformance/real.mjs
//   node conformance/diff.mjs --file conformance/real.jsonl
//
// Pinned so every number measured on it can be reproduced.
import { mkdirSync, writeFileSync } from "node:fs";

const FILES = [
  ["micromark/micromark", "6577c200155e9c6b85a42b26d298f8affc348f2f", "readme.md"],
  ["remarkjs/remark", "1146b3a274fc1f4607111e5d6607a3a769a0e89a", "readme.md"],
  ["rust-unofficial/awesome-rust", "e1b794dddffc9ab72d08b0b832867b930223bd31", "README.md"],
  ["github/cmark-gfm", "27d942c8b0a62d192f616e5bf3578f4b6a89e180", "test/spec.txt"],
  [
    "vercel/streamdown",
    "c5e44d84cfcc10427ae380863a562e8127ae3b86",
    "packages/streamdown/README.md",
  ],
  ["denoland/deno", "d7205acbed60f68414d1485407a5b50bc867131e", "README.md"],
  ["markdown-it/markdown-it", "a8c00d474ec9b304fad9b71c16d6fa8b4909279e", "README.md"],
  ["tldr-pages/tldr", "95062b79e30fc3539246399b1dbfcfabbccc79cf", "README.md"],
  ["vercel/next.js", "ba182aa1a68185844738ed60529cdf6ff6aac683", "packages/next/README.md"],
];
// Every chapter of the Rust book.
const BOOK = ["rust-lang/book", "1500248d8f230566e4ec9f27fcbb8fe9e2898ab1", "src"];

const raw = (repo, sha, path) => `https://raw.githubusercontent.com/${repo}/${sha}/${path}`;

async function text(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${response.status} ${url}`);
  return response.text();
}

const [repo, sha, dir] = BOOK;
const tree = JSON.parse(
  await text(`https://api.github.com/repos/${repo}/git/trees/${sha}?recursive=1`),
);
const chapters = tree.tree
  .filter((entry) => entry.path.startsWith(`${dir}/`) && entry.path.endsWith(".md"))
  .map((entry) => [repo, sha, entry.path]);

const out = new URL("real/", import.meta.url);
mkdirSync(out, { recursive: true });
const documents = await Promise.all(
  [...FILES, ...chapters].map(async ([repo, sha, path]) => {
    const name = `${repo.replace("/", "__")}__${path.replaceAll("/", "__")}`;
    const source = await text(raw(repo, sha, path));
    writeFileSync(new URL(name, out), source);
    return source;
  }),
);
writeFileSync(
  new URL("real.jsonl", import.meta.url),
  documents.map((source) => JSON.stringify(source)).join("\n") + "\n",
);
const bytes = documents.reduce((sum, source) => sum + source.length, 0);
console.log(`${documents.length} documents, ${(bytes / 1024) | 0} KB`);
