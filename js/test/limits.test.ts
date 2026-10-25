// Documents nested deeper than rill renders (rill::mdast::MAX_DEPTH) are left to
// Streamdown, and nothing a document does may trap the WebAssembly: a trap leaves the
// instance unusable for every later render.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { initRill } from "../src/index.ts";

const rill = await initRill(readFileSync(new URL("../wasm/rill_bg.wasm", import.meta.url)));
const options = { prefix: "", linkSafety: true, ssr: false, overrides: "" };

const DEEP = [
  "> ".repeat(3000) + "x",
  "- ".repeat(3000) + "x",
  "*".repeat(5000) + "a" + "*".repeat(5000),
  "<b>".repeat(3000) + "x" + "</b>".repeat(3000),
  "a " + "<sup>".repeat(3000) + "x",
];
// Deep, but flat in the tree (an image's alt is text): rendered.
const NESTED_IMAGES = "![".repeat(30000) + "a" + "](u)".repeat(30000);

test("deep nesting is left to Streamdown, and rill keeps rendering", () => {
  for (const source of DEEP) {
    assert.equal(rill.markup!(source, {}, options), "too-deep", source.slice(0, 20));
    rill.definitions!(source, {});
    rill.splitsAt!(source, {}, source.length >> 1);
    assert.equal(typeof rill.remend!(source), "string");
    assert.equal(typeof rill.markup!(`- **ok** ${source.length}`, {}, options), "object", "rill still renders");
  }
  assert.equal(typeof rill.markup!(NESTED_IMAGES, {}, options), "object");
});
