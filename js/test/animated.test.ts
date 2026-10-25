// `animated` (Streamdown 2's word animation) on rill's fast path: streamed into one root,
// rill's DOM equals Streamdown's after every update and once done, delays included (one
// clock for both), with rill rendering the blocks.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { mount } from "./dom.ts";

const { Streamdown } = await import("streamdown");
const { initRill } = await import("../src/index.ts");
const { blockRenders, Streamdown: RillStreamdown } = await import("../src/streamdown.ts");

await initRill(readFileSync(new URL("../wasm/rill_bg.wasm", import.meta.url)));

let clock = 0;
performance.now = () => clock;

const ANSWER = `# Plan for the *week*

Here is what we ship, word by word, with a [link](https://example.com) and \`code\`.

- [x] first item with **bold**
- second item
  1. nested 中文字符 too

> A quote that keeps going
> over two lines.

| a | b |
|---|---|
| 1 | **2** |

![an image](https://example.com/a.png)

---

\`\`\`ts
const x = 1;
\`\`\`

Done.`;

async function stream(component: unknown, props: Record<string, unknown>, every: number) {
  const root = mount(component);
  const out: string[] = [];
  clock = 1000;
  for (let end = 1; end <= ANSWER.length; end += every) {
    clock += 17;
    out.push(await root.render(ANSWER.slice(0, end), { ...props, isAnimating: true }));
  }
  clock += 17;
  out.push(await root.render(ANSWER, { ...props, isAnimating: true }));
  out.push(await root.render(ANSWER, { ...props, isAnimating: false }));
  await root.unmount();
  return out;
}

for (const [name, animated] of [
  ["defaults", true],
  ["every option", { animation: "slideUp", duration: 220, easing: "linear", sep: "char", stagger: 9, maxBacklogMs: 100 }],
] as const) {
  test(`animated (${name}) streams Streamdown's DOM`, async () => {
    for (const every of [1, 7]) {
      const reference = await stream(Streamdown, { animated }, every);
      const before = blockRenders.rill;
      const actual = await stream(RillStreamdown, { animated }, every);
      assert.ok(blockRenders.rill > before, "rill rendered the blocks");
      assert.ok(reference.some((html) => html.includes("data-sd-animate")), "it animates");
      for (const [index, html] of reference.entries()) {
        assert.equal(actual[index], html, `update ${index} of ${reference.length} (every ${every})`);
      }
    }
  });
}
