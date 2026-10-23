// Documents with link definitions are one block in Streamdown; the drop-in renders them as
// pieces, each with only the definitions whose label it may reference (streamdown.ts).
// Random documents of labels, definitions, references and containers stream through both
// in a DOM, and the HTML must be the same after every update.
//   node --test test/pieces.test.ts            (CI: a few hundred documents)
//   FUZZ=20000 node --test test/pieces.test.ts
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { mount } from "./dom.ts";

const { Streamdown } = await import("streamdown");
const { initRill } = await import(process.env.RILL_SRC ? `${process.env.RILL_SRC}/index.ts` : "../src/index.ts");
const { Streamdown: RillStreamdown } = await import(process.env.RILL_SRC ? `${process.env.RILL_SRC}/streamdown.ts` : "../src/streamdown.ts");

await initRill(readFileSync(new URL("../wasm/rill_bg.wasm", import.meta.url)));

const PIECES = [
  "[a]",
  "[A  b]",
  "[a\nb]",
  "[x][a]",
  "[i][b]",
  "[a][]",
  "[ß]",
  "[SS]",
  "[a\\]b]",
  "[c]",
  "\n\n[a]: /u\n\n",
  "\n\n[a b]: /v 'title'\n\n",
  "\n\n[b]: </w>\n\n",
  "\n\n[ss]: /ss\n\n",
  "\n\n[a\\]b]: /e\n\n",
  "\n\n[a|b]: /p\n\n",
  "\n\n[C]: /c\n",
  "| [a\\|b] | [c] |\n|-|-|\n| [a] | x |\n",
  "> [a\n> b] ",
  "\n- [b] ",
  "\n1. [A]\n",
  " text ",
  "\n\n",
  "\n",
  "*em* ",
  "`[a]` ",
  "*",
  "\n- ",
  "\n2. x",
  "\n===\n",
  "\n    indented\n",
  "\n```\n[a]\n",
  "\n<div>\n",
  "\n> ",
  "\n[d]:\n/d\n'title\nline'\n",
  "[d]",
];

/** A small seeded generator (mulberry32), so a failure can be replayed. */
function random(seed: number) {
  return () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const COUNT = Number(process.env.FUZZ ?? 300);

test("reference-link documents stream to Streamdown's DOM, update by update", async () => {
  const first = Number(process.env.SEED ?? 1);
  for (let seed = first; seed < first + COUNT; seed += 1) {
    const theirs = mount(Streamdown);
    const ours = mount(RillStreamdown);
    const next = random(seed);
    const length = 3 + Math.floor(next() * 10);
    let text = "";
    for (let index = 0; index < length; index += 1) {
      text += PIECES[Math.floor(next() * PIECES.length)];
    }
    // A text that has a definition at all, streamed a few units at a time.
    if (!text.includes("]:")) text += "\n\n[a]: /u";
    const step = 1 + Math.floor(next() * 6);
    for (let end = step; ; end = Math.min(end + step, text.length)) {
      const prefix = text.slice(0, end);
      const want = await theirs.render(prefix, { isAnimating: true });
      const got = await ours.render(prefix, { isAnimating: true });
      assert.equal(got, want, `seed ${seed}, ${JSON.stringify(prefix)}`);
      if (end === text.length) break;
    }
    await theirs.unmount();
    await ours.unmount();
  }
});
