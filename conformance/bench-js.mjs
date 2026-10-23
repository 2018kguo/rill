// What a page pays per render: Streamdown's JavaScript pipeline against rill's package
// (`rill.render`: WebAssembly, decoding, and the JavaScript tail a raw-HTML or math
// document still needs). Build the WebAssembly first (`npm run wasm`).
//
//   node conformance/bench-js.mjs              bundled examples, a link-heavy answer, streaming
//   node conformance/bench-js.mjs --real       also conformance/real.jsonl (node conformance/real.mjs)
import { readdirSync, readFileSync } from "node:fs";

import { initRill } from "../js/src/index.ts";
import { unified } from "unified";

import { rehypeMathFragments } from "../js/src/math.ts";
import { mathPlugins, processor, tails } from "./pipeline.mjs";

const rill = await initRill(readFileSync(new URL("../js/wasm/rill_bg.wasm", import.meta.url)));

const streamdown = (source) => processor.runSync(processor.parse(source), source);

/** rill's render with its own, empty KaTeX cache: every timed run starts cold, and a
 * streamed answer shares one cache across its updates, as a chat would. */
function freshRill() {
  const mathTail = unified().use(rehypeMathFragments(mathPlugins));
  return (source) => {
    const { kind, tree } = rill.render(source);
    if (kind === "math") return mathTail.runSync(tree, source);
    const tail = tails[kind];
    return tail ? tail.runSync(tree, source) : tree;
  };
}

/** Median milliseconds of `run`, after warming up, over at least 200 ms or 15 runs. */
function time(run) {
  for (let index = 0; index < 3; index += 1) run();
  const samples = [];
  const start = performance.now();
  while (samples.length < 15 || performance.now() - start < 200) {
    const begin = performance.now();
    run();
    samples.push(performance.now() - begin);
  }
  samples.sort((a, b) => a - b);
  return samples[samples.length >> 1];
}

function row(label, source, run = (render) => render(source)) {
  const js = time(() => run(streamdown));
  const rs = time(() => run(freshRill()));
  const kb = (source.length / 1024).toFixed(0).padStart(5);
  console.log(
    `${label.padEnd(34)} ${kb} KB  ${js.toFixed(2).padStart(9)} ms  ${rs.toFixed(2).padStart(8)} ms  ${(js / rs).toFixed(1).padStart(6)}×`,
  );
}

const dir = new URL("../examples/", import.meta.url);
const examples = readdirSync(dir)
  .filter((name) => name.endsWith(".md"))
  .sort()
  .map((name) => [name, readFileSync(new URL(name, dir), "utf8")]);
// The streaming example ends in an open code fence, which would swallow every repeat
// after it; math and raw HTML send a document to a JavaScript tail.
const closed = examples.filter(([name]) => !name.includes("streaming"));
const pure = closed.filter(([name]) => !name.includes("math") && !name.includes("html"));
const join = (list) => list.map(([, source]) => source).join("\n\n");
const answer = join(pure);
const withMath = join(closed.filter(([name]) => !name.includes("html")));
const withHtml = join(closed.filter(([name]) => !name.includes("math")));
const repeat = (source) => source.repeat(Math.floor(100_000 / source.length));
const links = Array.from(
  { length: 600 },
  (_, index) =>
    `See [source ${index}](https://example.com/docs/${index}?ref=chat#section) and ![figure](/img/${index}.png).`,
).join("\n\n");

console.log(
  `Node ${process.versions.node}, ${process.arch}. Median of runs, each with an empty KaTeX cache.`,
);
console.log(
  `${"".padEnd(34)} ${"size".padStart(8)}  ${"Streamdown".padStart(12)}  ${"rill".padStart(11)}  speedup`,
);
row("Answer, Markdown only", answer);
row("Answer with math", withMath);
row("Answer with raw HTML", withHtml);
row("Long answer, Markdown only", repeat(answer));
row("Long answer with math", repeat(withMath));
// Every formula distinct, so nothing is cached within the render either.
const formulas = Array.from(
  { length: 300 },
  (_, index) =>
    `Step ${index}: $$\\sum_{k=0}^{${index}} \\frac{x^k}{k!} \\approx e^{${index}}$$ holds.`,
).join("\n\n");
row("300 distinct formulas", formulas);
row("Long answer with raw HTML", repeat(withHtml));
row("Link-heavy answer (1,200 URLs)", links);

// Streaming: an answer arrives 24 characters at a time and every prefix is rendered,
// as a chat UI does on each update. Times are the total for the whole stream.
function streamed(label, source) {
  const prefixes = [];
  for (let end = 24; end < source.length; end += 24) prefixes.push(source.slice(0, end));
  prefixes.push(source);
  row(`${label}, ${prefixes.length} renders`, source, (render) => {
    for (const prefix of prefixes) render(prefix);
  });
}
streamed("Streamed answer", answer.repeat(4));
streamed("Streamed answer with math", withMath.repeat(3));
streamed("Streamed, 40 distinct formulas", formulas.split("\n\n").slice(0, 40).join("\n\n"));

if (process.argv.includes("--real")) {
  const documents = readFileSync(new URL("real.jsonl", import.meta.url), "utf8")
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line));
  const all = documents.join("\n\n");
  row(`Real-world docs (${documents.length}), one by one`, all, (render) => {
    for (const source of documents) render(source);
  });
}
