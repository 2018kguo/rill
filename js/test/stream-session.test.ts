// rill-stream's session and DOM renderer against Streamdown while text streams: a
// depth-first walk appends one piece at a time and cuts back (a new text) before the
// next, and after every update that does not fall back, the root must hold
// Streamdown's DOM exactly (HTML and text nodes).
//   node --test test/stream-session.test.ts
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { test } from "node:test";

import { mount } from "./dom.ts";

const { Streamdown } = await import("streamdown");
const { createStreamRenderer } = await import("../src/stream-dom.ts");
const { Stream, streamMemory, streamTags } = createRequire(import.meta.url)("../stream-wasm/rill_stream.js");

const ROOT_CLASS = "space-y-4 whitespace-normal [&>*:first-child]:mt-0 [&>*:last-child]:mb-0";

function textNodes(node: Node): string {
  let out = "";
  for (let child = node.firstChild; child; child = child.nextSibling) {
    out += child.nodeType === 3 ? `[${child.nodeValue}]` : textNodes(child);
  }
  return out;
}

const LONG = `${"word ".repeat(60)}`;
const ALPHABETS: [name: string, pieces: string[], depth: number, controls?: boolean][] = [
  ["inline", [..."*_`a \n"], 6],
  ["blocks", ["# ", "---", "a", " ", "\n", "\n\n", "**", "  \n"], 5],
  ["paragraphs", [LONG, "\n\n", "*", "`", "\n", "# x"], 5],
  ["lists", ["- ", "* ", "1. ", "a", " ", "\n", "  ", "*", "\n\n"], 5],
  ["unicode", ["*", "`", "é", "😀", "“", " ", "\n\n", "~"], 5],
  ["mixed", ["**", "*", "_", "`", "a", " ", "\n", "# ", "\\", "é"], 5],
  ["controls", ["```js\n", "a", "\n", "```", "| a |\n|--|\n", "| b |", "\n\n"], 5, true],
  ["footnotes", ["[^a]", "[^a]: ", "[c]", "[c]: /u", "x", "\n", "\n\n", "    ", "- "], 5],
  ["raw breaks", ["<", "br", "BR", ">", " /", "a", "|", "\n", "|-|\n"], 5],
];

/** Each tag's attributes in name order: an element updated in place (as React updates
 * one, new attributes last) and one built fresh are the same element. The drop-in test
 * checks the order, streaming as Streamdown does. */
const sortAttributes = (html: string) =>
  html.replace(/<([a-z][\w-]*)((?:\s[\w:-]+(?:="[^"]*")?)*)(\s*\/?)>/g, (_, tag: string, attributes: string, end: string) =>
    `<${tag}${(attributes.match(/\s[\w:-]+(?:="[^"]*")?/g) ?? []).sort().join("")}${end}>`
  );

for (const [name, pieces, depth, controls = false] of ALPHABETS) {
/** A session rendering for Streamdown's `controls` and `isAnimating`. */
const streamWith = (controls: boolean, animating: boolean) => {
  const stream = new Stream();
  stream.setOptions(controls, animating);
  return stream;
};

  test(`a streaming session shows Streamdown's DOM after every update (${name})`, async () => {
    const theirs = mount(Streamdown);
    const host = document.createElement("div");
    const root = document.createElement("div");
    root.className = ROOT_CLASS;
    host.append(root);
    const renderer = createStreamRenderer(root, streamWith(controls, true), { memory: streamMemory(), tags: streamTags() });
    let shown = 0;
    let updates = 0;
    const walk = async (prefix: string, left: number) => {
      for (const piece of pieces) {
        const text = prefix + piece;
        updates += 1;
        if (renderer.update(text)) {
          shown += 1;
          // While a block streams its text may sit in several adjacent text nodes; the
          // HTML must match at every update, and the text nodes once normalized (as
          // each block is when it is final).
          const settled = host.cloneNode(true) as Element;
          settled.normalize();
          const got = `${host.innerHTML}\n${textNodes(settled)}`;
          // (With controls, a fresh Streamdown: its memoized code blocks keep the
          // `data-incomplete` of a longer text the walk cut back from.)
          const fresh = controls ? mount(Streamdown) : theirs;
          await fresh.render(text, { isAnimating: true, controls });
          // (Streamdown's own text nodes normalized too: a dropped footnote item leaves
          // two line endings apart.)
          const theirsSettled = fresh.host.cloneNode(true) as Element;
          theirsSettled.normalize();
          const want = `${fresh.host.innerHTML}\n${textNodes(theirsSettled)}`;
          if (controls) await fresh.unmount();
          if (sortAttributes(got) !== sortAttributes(want)) {
            let at = 0;
            while (got[at] === want[at]) at += 1;
            assert.fail(`${JSON.stringify(text)}\n  ours:       …${JSON.stringify(got.slice(at - 60, at + 80))}\n  Streamdown: …${JSON.stringify(want.slice(at - 60, at + 80))}`);
          }
        }
        if (left > 1) await walk(text, left - 1);
      }
    };
    await walk("", depth);
    renderer.dispose();
    await theirs.unmount();
    console.log(`${name}: ${shown}/${updates} updates shown by rill-stream`);
    assert.ok(shown > updates / 2);
  });
}
