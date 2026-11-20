// rill-stream against Streamdown: every document over small alphabets either falls back or
// renders Streamdown's exact DOM (the HTML and its text nodes), built as the DOM renderer
// builds it from a fresh session.
//   node --test test/stream.test.ts
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { test } from "node:test";

import { mount } from "./dom.ts";

const { Streamdown } = await import("streamdown");
const { createStreamRenderer } = await import("../src/stream-dom.ts");
const { Stream, streamMemory, streamTags } = createRequire(import.meta.url)("../stream-wasm/rill_stream.js");

const ROOT = '<div class="space-y-4 whitespace-normal [&amp;>*:first-child]:mt-0 [&amp;>*:last-child]:mb-0">';

function textNodes(node: Node): string {
  let out = "";
  for (let child = node.firstChild; child; child = child.nextSibling) {
    out += child.nodeType === 3 ? `[${child.nodeValue}]` : textNodes(child);
  }
  return out;
}
const scratch = document.createElement("div");
const module = { memory: streamMemory(), tags: streamTags() };
function ours(text: string): string | undefined {
  scratch.innerHTML = `${ROOT}</div>`;
  const session = new Stream();
  const renderer = createStreamRenderer(scratch.firstChild as Element, session, module);
  const shown = renderer.update(text);
  renderer.dispose();
  return shown ? `${scratch.innerHTML}\n${textNodes(scratch)}` : undefined;
}

// The last field: the share of documents that must take the fast engine (default half).
const ALPHABETS: [name: string, pieces: string[], depth: number, fast?: number][] = [
  ["emphasis", [..."*_a \n"], 6],
  ["code and escapes", [..."`\\*a \n"], 5],
  ["blocks", ["# ", "#", "---", "***", "a", " ", "\n", "  ", "*"], 4],
  ["punctuation", [..."*_.!a \n"], 5],
  ["mixed", ["**", "*", "_", "`", "a", " ", "\n", "# ", "  \n", "\\"], 5],
  ["lists", ["- ", "* ", "1. ", "2) ", "a", " ", "\n", "  ", "*"], 5],
  ["list blocks", ["- ", "-", "# ", "---", "a", "\n", "\n\n", "  "], 5],
  ["unicode", ["*", "_", "é", "—", "“", " ", "a", "\n"], 5],
  ["astral", ["*", "_", "😀", "𝐀", " ", "a"], 6],
  ["autolinks", ["www", ".", "@", ":", "//", "`", "\\", "a", " "], 5],
  ["links", ["[", "]", "(", ")", "a", " ", "*", "<", "\n", "\\"], 5],
  ["destinations", ["[a](", "<", ">", ")", "(", "\"", "/", ":", "j", " "], 5],
  ["references", ["&", "amp", ";", "#", "x", "41", "[", "](u)", "a", "\\"], 4],
  // A letter after `<` is raw HTML, which takes the exact path for now.
  ["autolink brackets", ["<", ">", "a", ":", "@", ".", "b", "-", " "], 5, 0.4],
];

for (const [name, pieces, depth, fast = 0.5] of ALPHABETS) {
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
    assert.ok(rendered > documents * fast, "most documents should take the fast engine");
  });
}
