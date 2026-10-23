import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { code } from "@streamdown/code";
import { createElement, memo, type ComponentType } from "react";
import { renderToString } from "react-dom/server";
import remend from "remend";
import { Block as StreamdownBlock, Streamdown } from "streamdown";

import { initRill } from "../src/index.ts";
import {
  Block,
  blockRenders,
  Streamdown as RillStreamdown,
} from "../src/streamdown.ts";

const rill = await initRill(
  readFileSync(new URL("../wasm/rill_bg.wasm", import.meta.url))
);

const DOCUMENTS = [
  "# Title *em* **strong** ~~gone~~ `code` and a [link](https://example.com)",
  "- [x] done\n- [ ] open\n  1. nested\n\n> quote",
  "| a | b |\n|:--|--:|\n| 1 | **2** |\n| x | [y](https://example.com) |",
  "```ts\nconst x: number = 1;\n```\n\nText after.",
  "A footnote[^1].\n\n[^1]: The note.",
  "Streaming **bold and [a link](https://exa",
];

const render = (component: unknown, source: string, props = {}) =>
  renderToString(
    createElement(component as ComponentType<Record<string, unknown>>, props, source)
  );

// Streamdown's code block loads its highlighted body lazily: render once and let it
// load, so both sides see the same module state (as conformance/dom/ssr.tsx does).
for (const source of DOCUMENTS) render(Streamdown, source, { plugins: { code } });
await new Promise((resolve) => setTimeout(resolve, 100));

test("server rendering equals Streamdown's, with and without the code plugin", () => {
  for (const plugins of [undefined, { code }]) {
    for (const source of DOCUMENTS) {
      assert.equal(render(RillStreamdown, source, { plugins }), render(Streamdown, source, { plugins }));
    }
  }
  assert.ok(blockRenders.rill > 0, "rill rendered blocks");
});

test("a BlockComponent wrapping rill's Block renders as one wrapping Streamdown's", () => {
  const wrap = (Inner: ComponentType<Record<string, unknown>>) =>
    memo((props: Record<string, unknown>) => createElement(Inner, props));
  const ours = wrap(Block as never);
  const theirs = wrap(StreamdownBlock as never);
  for (const source of DOCUMENTS) {
    assert.equal(
      render(RillStreamdown, source, { BlockComponent: ours }),
      render(Streamdown, source, { BlockComponent: theirs })
    );
  }
});

test("rill's remend follows a stream as the package heals each prefix", () => {
  const answer = DOCUMENTS.join("\n\n");
  for (let end = 1; end <= answer.length; end += 1) {
    const prefix = answer.slice(0, end);
    assert.equal(rill.remend?.(prefix), remend(prefix), JSON.stringify(prefix));
  }
});
