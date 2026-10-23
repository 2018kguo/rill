// Differential check: rill's remend (crates/rill/src/remend.rs, through the WebAssembly)
// against the remend 1.4.0 package Streamdown 2.7.1 runs, with default options.
//
//   node conformance/remend.mjs                  corpora, and prefixes of each document
//   node conformance/remend.mjs --fuzz 200000    plus seeded random marker soups
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import remend from "remend";

import { initRill } from "../js/src/index.ts";

const repo = new URL("..", import.meta.url).pathname;
// The package's remend: Rust keeps the text, so a stream of prefixes sends only what
// each one appends.
const rill = await initRill(readFileSync(join(repo, "js/wasm/rill_bg.wasm")));
const rillRemend = (text) => rill.remend(text);
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
  "*", "**", "***", "_", "__", "~", "~~", "`", "``", "```", "~~~", "$", "$$", "[", "]", "(", ")",
  "](", "![", "<", ">", "<b", "</", "\\", "\\(", "\\[", "\\]", "\\)", "-", "--", "---", "=", "==",
  "+", "1.", "2)", "#", " ", "  ", "\t", "\n", "\n\n", "\r\n", "a", "word", "中", "😀", "x_y",
  "20~25", "- > 5", "1. >= $3", "> ", "    ", "\n- ", "\n> ", "http://a.b/c_d", "<a href=\"x_y\">",
  "é", " ", " ", "*a*", "**b**", "`c`",
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
    const length = 1 + Math.floor(random() * 16);
    for (let piece = 0; piece < length; piece += 1) {
      source += PIECES[Math.floor(random() * PIECES.length)];
    }
    yield source;
  }
}

let checked = 0;
const failures = [];
const check = (text) => {
  // Text with lone surrogates (a prefix that splits an emoji) stays in JavaScript.
  if (!text.isWellFormed()) return;
  checked += 1;
  const expected = remend(text);
  const actual = rillRemend(text);
  if (expected !== actual && failures.length < 2000) failures.push({ text, expected, actual });
};

for (const document of documents) {
  check(document);
  // A streamed answer: prefixes in order, as the package sees them (about 1500 per
  // document for the long ones).
  const step = Math.max(1, Math.floor(document.length / 1500));
  for (let end = 1; end < document.length; end += step) check(document.slice(0, end));
}
const count = Number(option("--fuzz") ?? 0);
for (const text of fuzz(count, Number(option("--seed") ?? 7))) check(text);

console.log(`${checked - failures.length}/${checked} identical (remend 1.4.0)`);
for (const { text, expected, actual } of failures.sort((a, b) => a.text.length - b.text.length).slice(0, 8)) {
  console.log(`  ${JSON.stringify(text).slice(0, 160)}`);
  console.log(`    remend ${JSON.stringify(expected).slice(-120)}`);
  console.log(`    rill   ${JSON.stringify(actual).slice(-120)}`);
}
process.exitCode = failures.length ? 1 : 0;
