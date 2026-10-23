// Differential check: rill's block splitter (js/src/blocks.ts) against Streamdown
// 2.7.1's parseMarkdownIntoBlocks, fed the same sequence of texts: every document, its
// prefixes in order (a stream), and seeded random documents streamed the same way; each
// stream raw and as Streamdown splits it while streaming, healed by remend (whose
// closing markers end each text differently), and backwards (texts that shrink).
//
//   node conformance/blocks.mjs [--fuzz 2000] [--seed 7]
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import remend from "remend";
import { parseMarkdownIntoBlocks as streamdown } from "streamdown";

import { parseMarkdownIntoBlocks as rill } from "../js/src/blocks.ts";

const repo = new URL("..", import.meta.url).pathname;
const args = process.argv.slice(2);
const option = (name) => {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : undefined;
};
const markdown = (directory) =>
  readdirSync(directory)
    .filter((file) => file.endsWith(".md"))
    .map((file) => readFileSync(join(directory, file), "utf8"));
const documents = [
  ...markdown(join(repo, "examples")),
  ...markdown(join(repo, "conformance/dom/chat")),
  ...readFileSync(join(repo, "conformance/real.jsonl"), "utf8")
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line)),
];

const PIECES = [
  "# ", "## ", "- ", "1. ", "> ", "```", "~~~", "    ", "| a | b |\n|---|---|\n", "| 1 | 2 |",
  "$$", "<div>", "</div>", "<details>", "</details>", "[^1]", "[^1]: note", "[a]: /u", "[a]",
  "---", "***", "===", "text", "**bold**", "\n", "\n\n", "\r\n", " ", "`", "<br>", "<!-- c -->",
  "[x](y)", "]", "[", ":", "中文", "😀",
];

function* fuzz(count, seed) {
  let state = seed >>> 0 || 1;
  const random = () => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    return (state >>> 0) / 4294967296;
  };
  for (let index = 0; index < count; index += 1) {
    let source = "";
    const length = 1 + Math.floor(random() * 30);
    for (let piece = 0; piece < length; piece += 1) {
      source += PIECES[Math.floor(random() * PIECES.length)];
    }
    yield source;
  }
}

let checked = 0;
const failures = [];
const check = (text) => {
  checked += 1;
  const expected = JSON.stringify(streamdown(text));
  const actual = JSON.stringify(rill(text));
  if (expected !== actual && failures.length < 100) failures.push({ text, expected, actual });
};
const stream = (text, step) => {
  const ends = [];
  for (let end = 1; end < text.length; end += step) ends.push(end);
  ends.push(text.length);
  for (const end of ends) check(text.slice(0, end));
  for (const end of ends) check(remend(text.slice(0, end)));
  for (const end of ends.reverse()) check(text.slice(0, end));
};

for (const document of documents) stream(document, Math.max(1, Math.floor(document.length / 1500)));
for (const text of fuzz(Number(option("--fuzz") ?? 2000), Number(option("--seed") ?? 7))) {
  stream(text, 1);
}

console.log(`${checked - failures.length}/${checked} identical (parseMarkdownIntoBlocks)`);
for (const { text, expected, actual } of failures.slice(0, 5)) {
  console.log(`  ${JSON.stringify(text).slice(0, 160)}`);
  console.log(`    streamdown ${expected.slice(0, 200)}`);
  console.log(`    rill       ${actual.slice(0, 200)}`);
}
process.exitCode = failures.length ? 1 : 0;
