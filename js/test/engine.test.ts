// A custom build (examples/custom-build: math and three plugins of its own, with two
// arguments) through the drop-in: the same DOM as Streamdown with the JavaScript plugins
// the engine carries, streaming and in static mode, with rill rendering the blocks.
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { test } from "node:test";

import "./dom.ts";

const pkg = new URL("../../examples/custom-build/pkg/", import.meta.url);
if (!existsSync(new URL("custom_bg.wasm", pkg))) {
  throw new Error("Build the example first: sh examples/custom-build/build.sh");
}
(globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = false;

const { customEngine } = await import("../../examples/custom-build/engine.ts");
const { checkEngine } = await import("../src/conformance.ts");
const { renderToString } = await import("react-dom/server");
const { createElement } = await import("react");
const { Streamdown } = await import("streamdown");
const { Streamdown: RillStreamdown } = await import("../src/streamdown.ts");

await customEngine.init(readFileSync(new URL("custom_bg.wasm", pkg)));

const DOCUMENTS = [
  "Hi @ann and @bo_2, see #News and #release-notes :tada:",
  "# Launch :rocket:\n\n- ping @team #urgent\n- `@not #code`\n\n> [@quoted](https://x.com) #quote",
  "| who | tag |\n|-|-|\n| @ann | #a |\n| **@bo** | #b-2 |",
  "Math $$x^2$$ and\n\n$$\n\\int_0^1 f\n$$\n\nthen #after and @me",
  "Math with a hash: $$a #b$$ #tag",
  "```js\n// @user #tag\n```\n\n:smile: a@b.co #x",
  "A note[^1] for @ann.\n\n[^1]: Footnote #tagged.",
];

for (const args of [["/users", true], [null, false], ["https://x.com/u", false]] as const) {
  test(`the custom build renders Streamdown's DOM with its plugins (${JSON.stringify(args)})`, async () => {
    const report = await checkEngine({
      engine: customEngine,
      args: [...args] as [string | null, boolean],
      documents: DOCUMENTS,
      stream: 9,
    });
    assert.deepEqual(report.differences.slice(0, 2), []);
    assert.equal(report.identical, report.renders);
    assert.ok(report.rillBlocks > report.streamdownBlocks * 4, JSON.stringify(report));
  });
}

test("server rendering with the custom build equals Streamdown's", () => {
  const args: [string, boolean] = ["/users", true];
  for (const mode of ["streaming", "static"] as const) {
    for (const source of DOCUMENTS) {
      assert.equal(
        renderToString(createElement(RillStreamdown, { engine: customEngine, engineArgs: args, mode }, source)),
        renderToString(createElement(Streamdown, { ...customEngine.plugins(...args), mode }, source)),
        source
      );
    }
  }
});

test("engine arguments compare by value", () => {
  assert.equal(customEngine.plugins("/u", true), customEngine.plugins("/u", true));
  assert.equal(customEngine.rill("/u", true), customEngine.rill("/u", true));
  assert.notEqual(customEngine.plugins("/u", true), customEngine.plugins("/u", false));
});
