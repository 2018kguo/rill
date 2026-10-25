// Every way a block leaves rill for Streamdown's own Block is reported: to the listener
// (`setFallbackListener`), and in development once per reason as a console warning.
// Production builds carry none of the warnings' text.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { mount } from "./dom.ts";

const { createElement } = await import("react");
const { renderToString } = await import("react-dom/server");
const { createMathPlugin, math } = await import("@streamdown/math");
const { initRill } = await import("../src/index.ts");
const { defineEngine } = await import("../src/engine.ts");
const { ABI } = await import("../src/core.ts");
const { resetFallbackLog } = await import("../src/fallback.ts");
const { Streamdown, setFallbackListener } = await import("../src/streamdown.ts");
const { customEngine } = await import("../../examples/custom-build/engine.ts");

await initRill(readFileSync(new URL("../wasm/rill_bg.wasm", import.meta.url)));
const customWasm = new URL("../../examples/custom-build/pkg/custom_bg.wasm", import.meta.url);
await customEngine.init(readFileSync(customWasm));

type Heard = { reason: string; detail: string | undefined };

/** The reasons heard and the warnings logged while `run` renders. */
async function hear(run: () => Promise<void> | void) {
  const heard: Heard[] = [];
  const warnings: string[] = [];
  const warn = console.warn;
  console.warn = (message: unknown) => warnings.push(String(message));
  resetFallbackLog();
  setFallbackListener((reason, detail) => heard.push({ reason, detail }));
  try {
    await run();
  } finally {
    setFallbackListener(null);
    console.warn = warn;
  }
  return { heard, reasons: new Set(heard.map((entry) => entry.reason)), warnings };
}

async function client(source: string, props: Record<string, unknown> = {}, steps: string[] = []) {
  const root = mount(Streamdown);
  for (const step of steps) await root.render(step, { ...props, isAnimating: true });
  await root.render(source, props);
  await root.unmount();
}

function noop() {}
function remarkOwn() {
  return noop;
}
function rehypeOwn() {
  return noop;
}
const Fallback = () => null;

// [reason, document, props] rendered in the browser renderer.
const CASES: [string, string, Record<string, unknown>][] = [
  ["remarkPlugins", "Some *text*.", { remarkPlugins: [remarkOwn] }],
  ["rehypePlugins", "Some *text*.", { rehypePlugins: [rehypeOwn] }],
  ["allowedTags", "Some <mention user_id=\"1\">x</mention>.", { allowedTags: { mention: ["user_id"] } }],
  ["literalTagContent", "Some *text*.", { literalTagContent: ["mention"] }],
  ["disableAutolinkProtocols", "mail a@b.co", { disableAutolinkProtocols: ["mailto"] }],
  ["urlTransform", "[a](b)", { urlTransform: (url: string) => url }],
  ["allowElement", "Some *text*.", { allowElement: () => true }],
  ["allowedElements", "Some *text*.", { allowedElements: ["p", "em"] }],
  ["disallowedElements", "Some *text*.", { disallowedElements: ["em"] }],
  ["skipHtml", "Some *text*.", { skipHtml: true }],
  ["unwrapDisallowed", "Some *text*.", { unwrapDisallowed: true }],
  ["remarkRehypeOptions", "Some *text*.", { remarkRehypeOptions: {} }],
  ["fallbackComponent", "Some *text*.", { fallbackComponent: Fallback }],
  ["static-dir", "Some *text*.", { mode: "static", dir: "rtl" }],
  ["engine-lists", "Some *text*.", { engine: customEngine, engineArgs: ["/u", true], rehypePlugins: [rehypeOwn] }],
  ["math-twice", "Some $$x$$.", { engine: customEngine, engineArgs: ["/u", true], plugins: { math } }],
  ["raw-html", "<details><summary>a</summary>b</details>", {}],
  ["too-deep", `${"> ".repeat(300)}x`, {}],
  ["too-long", "a".repeat(140000), {}],
  ["surrogates", "a\uD800b", {}],
  ["plugin-declined", "Math with a hash: $$a #b$$ #tag", { engine: customEngine, engineArgs: ["/u", true] }],
];

for (const [reason, source, props] of CASES) {
  test(`reports ${reason}`, async () => {
    const { reasons, warnings } = await hear(() => client(source, props));
    assert.ok(reasons.has(reason), `${reason}: heard ${[...reasons].join(", ")}`);
    const lines = warnings.filter((warning) => warning.startsWith("[rill]"));
    assert.ok(
      lines.some((line) => line.includes("slow path") && line.includes("#fallbacks")),
      lines.join("\n")
    );
  });
}

test("names the plugins rill does not know", async () => {
  const { heard, warnings } = await hear(() => client("x", { rehypePlugins: [rehypeOwn] }));
  assert.equal(heard[0]?.detail, "rehypeOwn");
  assert.match(warnings.join("\n"), /\[rill\] rehypePlugins given \(rehypeOwn\): these messages render with Streamdown \(slow path\)\./);
});

test("logs each reason once per page, and the listener hears every one", async () => {
  const { heard, warnings } = await hear(async () => {
    await client("a\n\nb\n\nc", { rehypePlugins: [rehypeOwn] });
    await client("d", { rehypePlugins: [rehypeOwn] });
  });
  assert.equal(warnings.length, 1);
  assert.ok(heard.length >= 2, String(heard.length));
});

test("reports nothing for documents rill renders", async () => {
  const { heard, warnings } = await hear(() =>
    client("# Title\n\n- a [link](https://x.com)\n\n| a |\n|-|\n| 1 |\n\n```js\nx\n```", {}, ["# Ti"])
  );
  assert.deepEqual(heard, []);
  assert.deepEqual(warnings, []);
});

test("reports a streaming fence beside the page's own components or diagram renderers", async () => {
  const Code = (props: Record<string, unknown>) => createElement("code", props);
  const own = await hear(() => client("```js\nco", { components: { code: Code } }, ["```js\nco"]));
  assert.ok(own.reasons.has("incomplete-fence-components"), [...own.reasons].join());
  const renderers = { renderers: [{ language: "x", component: () => null }] };
  const diagram = await hear(() => client("```x\nco", { plugins: renderers }, ["```x\nco"]));
  assert.ok(diagram.reasons.has("incomplete-diagram"), [...diagram.reasons].join());
});

test("reports math a build leaves to a KaTeX it was not given", async () => {
  // The custom build's math, without the `katex` entry its engine names.
  const engine = defineEngine({
    glue: () => import("../../examples/custom-build/pkg/custom.js"),
    wasm: () => customWasm,
    plugins: (...args: [string | null, boolean]) => customEngine.plugins(...args),
  });
  await engine.init(readFileSync(customWasm));
  const { reasons } = await hear(() => client("Math $$x$$", { engine, engineArgs: ["/u", true] }));
  assert.ok(reasons.has("math-without-katex"), [...reasons].join());
});

/** An engine whose WebAssembly answers every render with `fail`. */
function brokenEngine(fail: () => never) {
  const glue = {
    rillAbi: () => ABI,
    renderHast: fail,
    renderMarkup: fail,
    pieces: () => false,
    definitions: fail,
    splitsAt: fail,
    remendStream: fail,
    remendMiddle: fail,
    default: async () => undefined,
    initSync: () => undefined,
  };
  return defineEngine({
    glue: async () => glue as never,
    wasm: () => "unused.wasm",
    plugins: () => customEngine.plugins(null, false),
  });
}

test("reports the WebAssembly loading, failing to load, crashing and erring", async () => {
  const pending = defineEngine({
    glue: () => new Promise<never>(() => {}),
    wasm: () => "pending.wasm",
    plugins: () => customEngine.plugins(null, false),
  });
  const loading = await hear(() => client("x", { engine: pending }));
  assert.ok(loading.reasons.has("loading"));
  assert.deepEqual(loading.warnings.filter((line) => line.startsWith("[rill]")), [], "not logged");

  const failing = defineEngine({
    glue: () => Promise.reject(new Error("offline")),
    wasm: () => "failing.wasm",
    plugins: () => customEngine.plugins(null, false),
  });
  const failed = await hear(async () => {
    await client("x", { engine: failing });
    await new Promise((resolve) => setTimeout(resolve, 10));
    await client("y", { engine: failing });
  });
  assert.ok(failed.reasons.has("load-failed"), [...failed.reasons].join());

  const trapping = brokenEngine(() => {
    throw new WebAssembly.RuntimeError("unreachable");
  });
  await trapping.init(new Uint8Array());
  const trapped = await hear(() => client("x", { engine: trapping }));
  assert.ok(trapped.reasons.has("trapped"), [...trapped.reasons].join());

  const erring = brokenEngine(() => {
    throw new Error("bad input");
  });
  await erring.init(new Uint8Array());
  const errored = await hear(() => client("x", { engine: erring }));
  assert.deepEqual(errored.heard.find((entry) => entry.reason === "error")?.detail, "bad input");
});

test("reports server rendering without the WebAssembly", async () => {
  const cold = defineEngine({
    glue: () => new Promise<never>(() => {}),
    wasm: () => "cold.wasm",
    plugins: () => customEngine.plugins(null, false),
  });
  const window = globalThis.window;
  // `renderToString` on a server: no window.
  delete (globalThis as Record<string, unknown>).window;
  try {
    const { reasons } = await hear(() => {
      renderToString(createElement(Streamdown as never, { engine: cold }, "x"));
    });
    assert.ok(reasons.has("server"), [...reasons].join());
  } finally {
    (globalThis as Record<string, unknown>).window = window;
  }
});

/** js/src/streamdown.ts bundled and minified, with `process.env.NODE_ENV` defined as
 * `mode`. */
function bundle(mode: string): string {
  const out = mkdtempSync(join(tmpdir(), "rill-bundle-"));
  try {
    const result = spawnSync(
      "bun",
      [
        "build",
        new URL("../src/streamdown.ts", import.meta.url).pathname,
        "--outdir",
        out,
        "--target",
        "browser",
        "--minify",
        "--packages",
        "external",
        "--define",
        `process.env.NODE_ENV=${JSON.stringify(mode)}`,
      ],
      { encoding: "utf8" }
    );
    assert.equal(result.status, 0, result.stderr);
    return readFileSync(join(out, "streamdown.js"), "utf8");
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
}

test("a production build carries no warning text", { skip: !hasBun() && "needs bun" }, () => {
  const texts = ["slow path", "[rill]", "#fallbacks", "raw HTML that needs"];
  const development = bundle("development");
  for (const text of texts) assert.ok(development.includes(text), text);
  const production = bundle("production");
  for (const text of texts) {
    assert.ok(!production.includes(text), `production bundle holds ${JSON.stringify(text)}`);
  }
  // The listener stays.
  assert.match(production, /setFallbackListener/);
});

function hasBun(): boolean {
  return spawnSync("bun", ["--version"]).status === 0 || existsSync("/usr/local/bin/bun");
}
