// Server rendering: renderToString of rill's <Streamdown> against Streamdown 2.7.1's,
// document by document, for the corpora and both `plugins` configurations that server
// rendering touches (code blocks render through Streamdown's own CodeBlock there).
//
//   bun conformance/dom/ssr.tsx [--config default|code] [--variant default|prefix|...]
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { code } from "@streamdown/code";
import { createElement, type ComponentType } from "react";
import { renderToString } from "react-dom/server";
import { Streamdown } from "streamdown";

import { initRill } from "../../js/src/index.ts";
import { blockRenders, Streamdown as RillStreamdown } from "../../js/src/streamdown.ts";

const here = import.meta.dir;
const repo = join(here, "../..");
const args = process.argv.slice(2);
const option = (name: string, fallback: string) => {
  const index = args.indexOf(name);
  return index >= 0 ? (args[index + 1] ?? fallback) : fallback;
};
const config = option("--config", "default");
const plugins = config === "code" ? { code } : undefined;
const variants: Record<string, Record<string, unknown>> = {
  default: {},
  prefix: { prefix: "tw" },
  controls: { controls: false },
  nosafety: { linkSafety: { enabled: false } },
  rtl: { dir: "rtl" },
  streaming: { isAnimating: true, caret: "block" },
};
const variant = option("--variant", "default");
const props = { plugins, ...variants[variant] };

await initRill(readFileSync(join(repo, "js/wasm/rill_bg.wasm")));

const markdown = (directory: string) =>
  readdirSync(directory)
    .filter((file) => file.endsWith(".md"))
    .sort()
    .map((file) => readFileSync(join(directory, file), "utf8"));
const corpora: Record<string, string[]> = {
  examples: markdown(join(repo, "examples")),
  real: readFileSync(join(repo, "conformance/real.jsonl"), "utf8")
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line) as string),
  chat: markdown(join(here, "chat")),
};

const render = (component: unknown, source: string) =>
  renderToString(createElement(component as ComponentType<Record<string, unknown>>, props, source));

// Streamdown's code block loads its highlighted body lazily: render everything once
// and let it load, so both sides see the same module state.
for (const documents of Object.values(corpora)) {
  for (const source of documents) render(Streamdown, source);
}
await new Promise((resolve) => setTimeout(resolve, 100));

let failed = 0;
for (const [name, documents] of Object.entries(corpora)) {
  let same = 0;
  for (const [index, source] of documents.entries()) {
    const reference = render(Streamdown, source);
    const actual = render(RillStreamdown, source);
    if (reference === actual) {
      same += 1;
      continue;
    }
    failed += 1;
    let at = 0;
    while (at < reference.length && reference[at] === actual[at]) at += 1;
    console.log(`  #${index} ${JSON.stringify(source).slice(0, 80)}`);
    console.log(`    reference …${reference.slice(Math.max(0, at - 100), at + 100)}`);
    console.log(`    actual    …${actual.slice(Math.max(0, at - 100), at + 100)}`);
  }
  console.log(`${same}/${documents.length} identical (${name}; SSR, config ${config}, variant ${variant})`);
}
console.log(`block renders: ${blockRenders.rill} by rill, ${blockRenders.streamdown} by Streamdown's Block`);
process.exitCode = failed ? 1 : 0;
