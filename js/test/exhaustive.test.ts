// Every document up to a few pieces long over small Markdown alphabets, streamed through
// both <Streamdown>s in one DOM each: a depth-first walk appends one piece at a time and
// cuts back to the parent before the next, so every document is rendered right after its
// prefix (and most also after a longer one was cut back), and the HTML and text nodes must
// match after every update.
//   node --test test/exhaustive.test.ts             (CI)
//   DEEPER=1 node --test test/exhaustive.test.ts    one piece longer (about ten times the work)
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { mount } from "./dom.ts";

const { Streamdown } = await import("streamdown");
const { initRill } = await import("../src/index.ts");
const { Streamdown: RillStreamdown, blockRenders } = await import("../src/streamdown.ts");

await initRill(readFileSync(new URL("../wasm/rill_bg.wasm", import.meta.url)));

// A paragraph long enough that the last block is handed to Streamdown as a caret view.
const LONG = `${"word ".repeat(450)}\n`;

const ALPHABETS: [name: string, pieces: string[], depth: number][] = [
  ["inline", [..."*_`[]()a \n"], 4],
  ["inline, more", [..."~$<>!\\|:/"].concat(["a", " ", "\n"]), 4],
  ["blocks", ["# ", "> ", "- ", "1. ", "```", "|", "-", "\n", "a", " "], 4],
  ["tables", ["| a ", "|", "-", ":", " ", "\n", "x", "**"], 4],
  ["definitions", ["[a]", "[a][]", "\n\n[a]: /u", "[^1]", "\n\n[^1]: n", "x", "\n", "`"], 5],
  ["caret", [LONG, "```", "~~~", "\n", "|", "-", ":", " ", "a"], 4],
];

for (const [name, pieces, depth] of ALPHABETS) {
  const limit = depth + (process.env.DEEPER ? 1 : 0);
  test(`every stream of up to ${limit} pieces renders Streamdown's DOM (${name})`, async () => {
    const theirs = mount(Streamdown);
    const ours = mount(RillStreamdown);
    let documents = 0;
    const walk = async (prefix: string, left: number) => {
      for (const piece of pieces) {
        const text = prefix + piece;
        const want = await theirs.render(text, { isAnimating: true });
        const got = await ours.render(text, { isAnimating: true });
        if (got !== want) {
          let at = 0;
          while (got[at] === want[at]) at += 1;
          assert.fail(
            `${JSON.stringify(text.replaceAll(LONG, "<LONG>"))}\n  rill:       …${got.slice(at - 80, at + 80)}\n  Streamdown: …${want.slice(at - 80, at + 80)}`
          );
        }
        documents += 1;
        if (left > 1) await walk(text, left - 1);
      }
    };
    await walk("", limit);
    await theirs.unmount();
    await ours.unmount();
    assert.ok(documents > 0);
  });
}

test("rill rendered the blocks", () => {
  assert.ok(blockRenders.rill > 0);
});
