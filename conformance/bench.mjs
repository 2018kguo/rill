// Times Streamdown's JavaScript pipeline on the document crates/rill/examples/bench.rs uses.
import { readdirSync, readFileSync } from "node:fs";

import { processor } from "./pipeline.mjs";

const dir = new URL("../examples/", import.meta.url);
const one = readdirSync(dir)
  .filter(
    (name) =>
      name.endsWith(".md") &&
      !name.includes("math") &&
      !name.includes("html") &&
      // Ends in an open code fence, which would swallow every repeat after it.
      !name.includes("streaming"),
  )
  .sort()
  .map((name) => readFileSync(new URL(name, dir), "utf8"))
  .join("\n\n");
const source = one.repeat(Math.max(1, Math.floor(100_000 / one.length)));
const time = (label, run) => {
  for (let i = 0; i < 5; i += 1) run();
  const samples = Array.from({ length: 30 }, () => {
    const start = performance.now();
    run();
    return performance.now() - start;
  }).sort((a, b) => a - b);
  console.log(`${label.padEnd(22)} ${samples[15].toFixed(3).padStart(8)} ms`);
};
console.log(`${Math.floor(source.length / 1024)} KB of Markdown`);
time("MDAST (remark-parse)", () => processor.parse(source));
time("HAST (full pipeline)", () => processor.runSync(processor.parse(source), source));
