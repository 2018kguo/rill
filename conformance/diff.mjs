// Differential check: rill's MDAST and HAST against remark/rehype's, positions included.
//
//   node diff.mjs                                    the examples in ../examples
//   node diff.mjs --alphabet '*_~[]a `' --length 5   every string over the characters
//   node diff.mjs --pieces '["- ","> ","```"]' --length 4
//   node diff.mjs --fuzz 2000 --seed 7               seeded random concatenations
//   node diff.mjs --file cases.jsonl                 one JSON string per line
//   node diff.mjs --config math                      a Streamdown `plugins` configuration
//                                                    (default, math, cjk, math+cjk)
import { spawnSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";

import { pipeline } from "./pipeline.mjs";

const args = process.argv.slice(2);
const option = (name) => {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : undefined;
};
const { processor, tails, flags } = pipeline(option("--config") ?? "default");

function* exhaustive(pieces, length) {
  let layer = [""];
  for (let size = 1; size <= length; size += 1) {
    layer = layer.flatMap((prefix) => pieces.map((piece) => prefix + piece));
    yield* layer;
  }
}

const FUZZ = [
  "*", "**", "_", "__", "~", "~~", "`", "``", "$$", "[", "]", "(", ")", "![", "<", ">",
  "\\", "&amp;", "|", "-", "1.", "2)", "#", " ", "  ", "\n", "\n\n", "\t", "a", "word",
  "https://x.com/p", "www.y.org", "a@b.com", "[^1]", "\n[^1]: note", "\n[ref]: /url",
  "[ref]", "\n> ", "\n- ", "\n1. ", "\n```", "\n    ", "<div>", "</div>", "<b>", "中文",
  "😀", "\n| a | b |\n| - | - |", "\r\n",
  // Streamdown 2.7.1: tel: and remend's streamdown: URLs, fence meta, CJK punctuation.
  "[t](tel:+1)", "](streamdown:incomplete-link)", "\n```js m=1", "。", "「",
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
    const length = 1 + Math.floor(random() * 24);
    for (let piece = 0; piece < length; piece += 1) {
      source += FUZZ[Math.floor(random() * FUZZ.length)];
    }
    yield source;
  }
}

function sources() {
  const length = Number(option("--length") ?? 4);
  if (option("--alphabet")) {
    const alphabet = option("--alphabet").replaceAll("\\n", "\n").replaceAll("\\t", "\t");
    return [...exhaustive([...alphabet], length)];
  }
  if (option("--pieces")) return [...exhaustive(JSON.parse(option("--pieces")), length)];
  if (option("--fuzz")) return [...fuzz(Number(option("--fuzz")), Number(option("--seed") ?? 1))];
  if (option("--file")) {
    return readFileSync(option("--file"), "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line));
  }
  const dir = new URL("../examples/", import.meta.url);
  return readdirSync(dir)
    .filter((name) => name.endsWith(".md"))
    .map((name) => readFileSync(new URL(name, dir), "utf8"));
}

function rill(cases) {
  const encoder = new TextEncoder();
  const chunks = cases.map((source) => {
    const bytes = encoder.encode(source);
    return Buffer.concat([Buffer.from(`${bytes.length}\n`), bytes]);
  });
  const binary = new URL("../target/release/examples/stdio", import.meta.url).pathname;
  const run = spawnSync(binary, flags, { input: Buffer.concat(chunks), maxBuffer: 1 << 30 });
  if (run.status !== 0) throw new Error(`stdio failed: ${run.stderr}`);
  return run.stdout.toString().trimEnd().split("\n").map((line) => JSON.parse(line));
}

// remark-math and remark-rehype hang `data` on MDAST nodes for later stages.
const plain = (tree) =>
  JSON.parse(JSON.stringify(tree, (key, value) => (key === "data" ? undefined : value)));

function firstDifference(expected, actual, path = "$") {
  if (Object.is(expected, actual)) return null;
  if (typeof expected !== "object" || typeof actual !== "object" || !expected || !actual) {
    return `${path}: expected ${JSON.stringify(expected)} got ${JSON.stringify(actual)}`;
  }
  const keys = new Set([...Object.keys(expected), ...Object.keys(actual)]);
  for (const key of keys) {
    const found = firstDifference(expected[key], actual[key], `${path}.${key}`);
    if (found) return found;
  }
  return null;
}

/** The first difference for each source, or null where rill matches. */
function compare(cases) {
  const results = rill(cases);
  return cases.map((source, index) => {
    const result = results[index];
    if (result.error) return `rill error: ${result.error}`;
    const mdast = processor.parse(source);
    const expectedHast = processor.runSync(processor.parse(source), source);
    const kind = result.hast.kind;
    if (!(kind in tails)) return `rill kind ${kind}`;
    const tail = tails[kind];
    const actualHast = tail ? tail.runSync(result.hast.tree, source) : result.hast.tree;
    return (
      firstDifference(plain(mdast), result.mdast, "mdast") ??
      firstDifference(plain(expectedHast), plain(actualHast), "hast")
    );
  });
}

/** Deletes code points while the source still differs: a minimal witness. */
function minimize(source) {
  let chars = [...source];
  for (;;) {
    const candidates = chars.map((_, index) => chars.toSpliced(index, 1).join(""));
    const differences = compare(candidates);
    const found = differences.findIndex(Boolean);
    if (found < 0) return chars.join("");
    chars = [...candidates[found]];
  }
}

const cases = sources();
const differences = compare(cases);
const failures = cases
  .map((source, index) => ({ source, difference: differences[index] }))
  .filter(({ difference }) => difference);
console.log(`${cases.length - failures.length}/${cases.length} identical`);
for (const { source, difference } of failures.slice(0, Number(option("--show") ?? 10))) {
  console.log(`  ${JSON.stringify(source)}\n    ${difference}`);
  if (args.includes("--minimize")) {
    const witness = minimize(source);
    console.log(`    minimal ${JSON.stringify(witness)}: ${compare([witness])[0]}`);
  }
}
process.exitCode = failures.length ? 1 : 0;
