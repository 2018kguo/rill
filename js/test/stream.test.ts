// rill-stream against Streamdown: every document over small alphabets either falls back or
// renders Streamdown's exact DOM (the HTML and its text nodes).
//   node --test test/stream.test.ts
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { test } from "node:test";

import { mount } from "./dom.ts";

const { Streamdown } = await import("streamdown");
const { initRill } = await import("../src/index.ts");
const rill = await initRill(readFileSync(new URL("../wasm/rill_bg.wasm", import.meta.url)));
const { renderStream } = createRequire(import.meta.url)("../stream-wasm/rill_stream.js") as {
  renderStream(source: string): string | undefined;
};

const ROOT = '<div class="space-y-4 whitespace-normal [&amp;>*:first-child]:mt-0 [&amp;>*:last-child]:mb-0">';

function textNodes(node: Node): string {
  let out = "";
  for (let child = node.firstChild; child; child = child.nextSibling) {
    out += child.nodeType === 3 ? `[${child.nodeValue}]` : textNodes(child);
  }
  return out;
}
const scratch = document.createElement("div");
function ours(text: string): string | undefined {
  const healed = rill.remend!(text) ?? text;
  const html = renderStream(healed);
  if (html === undefined) return undefined;
  scratch.innerHTML = `${ROOT}${html}</div>`;
  return `${scratch.innerHTML}\n${textNodes(scratch)}`;
}

const ALPHABETS: [name: string, pieces: string[], depth: number][] = [
  ["emphasis", [..."*_a \n"], 6],
  ["code and escapes", [..."`\\*a \n"], 5],
  ["blocks", ["# ", "#", "---", "***", "a", " ", "\n", "  ", "*"], 4],
  ["punctuation", [..."*_.!a \n"], 5],
  ["mixed", ["**", "*", "_", "`", "a", " ", "\n", "# ", "  \n", "\\"], 5],
  ["unicode", ["*", "_", "é", "—", "“", " ", "a", "\n"], 5],
];

for (const [name, pieces, depth] of ALPHABETS) {
  test(`rill-stream renders Streamdown's DOM or falls back (${name})`, async () => {
    const theirs = mount(Streamdown);
    let rendered = 0;
    let documents = 0;
    const walk = async (prefix: string, left: number) => {
      for (const piece of pieces) {
        const text = prefix + piece;
        documents += 1;
        const got = ours(text);
        if (got !== undefined) {
          rendered += 1;
          const want = await theirs.render(text, { controls: false });
          if (got !== want) {
            let at = 0;
            while (got[at] === want[at]) at += 1;
            assert.fail(`${JSON.stringify(text)}\n  ours:       …${got.slice(at - 60, at + 80)}\n  Streamdown: …${want.slice(at - 60, at + 80)}`);
          }
        }
        if (left > 1) await walk(text, left - 1);
      }
    };
    await walk("", depth);
    await theirs.unmount();
    console.log(`${name}: ${rendered}/${documents} rendered by rill-stream`);
    assert.ok(rendered > documents / 2, "most documents should take the fast engine");
  });
}
