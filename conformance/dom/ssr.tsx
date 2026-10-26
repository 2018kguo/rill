// Server rendering: renderToString of rill's <Streamdown> against Streamdown 2.7.1's,
// document by document, for the corpora and both `plugins` configurations that server
// rendering touches (code blocks render through Streamdown's own CodeBlock there).
//
//   bun conformance/dom/ssr.tsx [--config default|code] [--variant default|prefix|...]
//        [--corpus examples,real,chat,<file.md>] [--definitions] [--footnotes] [--every N]
//
// `--definitions` gives each document link definitions (and references to them), which
// make Streamdown render it as one block; `--footnotes` gives it footnotes (calls before
// and after the middle, a definition there and the rest at the end), which do too;
// `--every N` also renders every Nth prefix of each document, in order, as a stream does.
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { code } from "@streamdown/code";
import { createElement, type ComponentType } from "react";
import { renderToString } from "react-dom/server";
import { Streamdown } from "streamdown";

import { customEngine } from "../../examples/custom-build/engine.ts";
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
  // Streamdown 2's animated, on the server while streaming.
  animated: { isAnimating: true, animated: true },
  static: { mode: "static" },
  // The custom build (examples/custom-build), initialized on the server, against
  // Streamdown with its JavaScript plugins.
  engine: {},
  "engine-static": { mode: "static" },
};
const variant = option("--variant", "default");
// Delays after the first block's come from the clock: one that stands still for both.
if (variant === "animated") performance.now = () => 1000;
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
// A definition first, references and a late definition after the middle paragraph.
const withDefinitions = (source: string) => {
  const middle = source.indexOf("\n\n", source.length >> 1);
  const at = middle < 0 ? source.length : middle;
  return `[early]: https://example.com/early "Early"\n\n${source.slice(0, at)}\n\nSee [late], [the early one][early] and [early].\n\n[late]: https://example.com/late\n${source.slice(at)}`;
};
// Calls in the first paragraph and after the middle one (one called twice, one never
// defined), a definition at the middle (citing another) and the rest at the end.
const withFootnotes = (source: string) => {
  const first = source.indexOf("\n\n");
  const lead = first < 0 ? source.length : first;
  const middle = source.indexOf("\n\n", source.length >> 1);
  const at = middle < 0 ? source.length : Math.max(middle, lead);
  return `${source.slice(0, lead)}[^a]\n\nCited twice[^a], once[^b] and never defined[^none].\n${source.slice(lead, at)}\n\n[^b]: The middle note, citing [^c].\n\nAfter the middle[^c].\n${source.slice(at)}\n\n[^a]: The first note.\n[^c]: The last note, with *emphasis*.\n`;
};
const every = Number(option("--every", "0"));
const chosen = option("--corpus", "examples,real,chat").split(",");
for (const name of Object.keys(corpora)) if (!chosen.includes(name)) delete corpora[name];
for (const name of chosen) if (name.endsWith(".md")) corpora[name] = [readFileSync(name, "utf8")];
for (const [name, documents] of Object.entries(corpora)) {
  let list = args.includes("--definitions") ? documents.map(withDefinitions) : documents;
  if (args.includes("--footnotes")) list = list.map(withFootnotes);
  if (every > 0) {
    list = list.flatMap((source) => {
      const prefixes: string[] = [];
      for (let end = every; end < source.length; end += every) prefixes.push(source.slice(0, end));
      return [...prefixes, source];
    });
  }
  corpora[name] = list;
}

const ENGINE_ARGS: [string, boolean] = ["/users", true];
if (variant.startsWith("engine")) {
  await customEngine.init(readFileSync(join(repo, "examples/custom-build/pkg/custom_bg.wasm")));
}
const sideProps = (component: unknown) =>
  !variant.startsWith("engine")
    ? {}
    : component === Streamdown
      ? customEngine.plugins(...ENGINE_ARGS)
      : { engine: customEngine, engineArgs: ENGINE_ARGS };
const render = (component: unknown, source: string) =>
  renderToString(
    createElement(component as ComponentType<Record<string, unknown>>, { ...props, ...sideProps(component) }, source)
  );

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
