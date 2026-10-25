// The lists pages give `<Streamdown>` most often (conformance/... plugin survey):
// Streamdown's own plugins plus remark-breaks, remark-math with rehype-katex,
// rehype-slug and rehype-external-links render through rill, with the DOM Streamdown
// renders with them, also when a minifier renamed every function; anything else stays
// with Streamdown and says why.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";

import { mount } from "./dom.ts";

const { createMathPlugin, math } = await import("@streamdown/math");
const { defaultRehypePlugins, defaultRemarkPlugins, Streamdown } = await import("streamdown");
const { default: remarkGfm } = await import("remark-gfm");
const { initRill } = await import("../src/index.ts");
const { blockRenders, setFallbackListener, Streamdown: RillStreamdown } = await import(
  "../src/streamdown.ts"
);

await initRill(readFileSync(new URL("../wasm/rill_bg.wasm", import.meta.url)));

/** The plugin packages as an app's minified bundle has them (every name renamed). */
async function minified() {
  const dir = mkdtempSync(join(tmpdir(), "rill-min-"));
  try {
    const entry = join(dir, "entry.js");
    writeFileSync(
      entry,
      `export { default as remarkBreaks } from "remark-breaks";
export { default as remarkMath } from "remark-math";
export { default as rehypeKatex } from "rehype-katex";
export { default as rehypeSlug } from "rehype-slug";
export { default as rehypeExternalLinks } from "rehype-external-links";`
    );
    const root = new URL("../..", import.meta.url).pathname;
    const result = spawnSync(
      "bun",
      ["build", entry, "--outdir", join(dir, "out"), "--minify", "--target", "node"],
      { encoding: "utf8", cwd: root, env: { ...process.env, NODE_PATH: join(root, "node_modules") } }
    );
    if (result.status !== 0) throw new Error(result.stderr);
    return await import(pathToFileURL(join(dir, "out", "entry.js")).href);
  } finally {
    // Imported: the module stays loaded.
    setTimeout(() => rmSync(dir, { recursive: true, force: true }), 1000);
  }
}

const plain = {
  remarkBreaks: (await import("remark-breaks")).default,
  remarkMath: (await import("remark-math")).default,
  rehypeKatex: (await import("rehype-katex")).default,
  rehypeSlug: (await import("rehype-slug")).default,
  rehypeExternalLinks: (await import("rehype-external-links")).default,
};
const packed = await minified();
assert.notEqual(packed.remarkBreaks.name, "remarkBreaks", "the bundle renamed the plugins");

const chat = new URL("../../conformance/dom/chat/", import.meta.url);
const DOCUMENTS = [
  ...readdirSync(chat).sort().map((file) => readFileSync(new URL(file, chat), "utf8")),
  "Line one\nline two\r\nthree *a\nb* [c\nd](https://x.com)\n\n## Same\n\n## Same\n\n# Ünï 中 🚀",
  "Costs $5 and $10, math $x^2$ and $$\\frac{1}{2}$$ and\n\n$$\nE = mc^2\n$$\n",
  "[a](https://x.com) [b](//y.org) [c](/p) [d](mailto:a@b.co) <https://auto.example>",
];

const REMARK = Object.values(defaultRemarkPlugins);
const REHYPE = Object.values(defaultRehypePlugins);

type Case = [string, Record<string, unknown>];
const cases = (p: typeof plain): Case[] => [
  ["remark-breaks after Streamdown's", { remarkPlugins: [...REMARK, p.remarkBreaks] }],
  ["remark-gfm and remark-breaks", { remarkPlugins: [remarkGfm, p.remarkBreaks] }],
  [
    "remark-math and rehype-katex",
    { remarkPlugins: [...REMARK, p.remarkMath], rehypePlugins: [...REHYPE, p.rehypeKatex] },
  ],
  [
    "remark-math with $$ only",
    {
      remarkPlugins: [...REMARK, [p.remarkMath, { singleDollarTextMath: false }]],
      rehypePlugins: [...REHYPE, [p.rehypeKatex, { output: "html" }]],
    },
  ],
  ["rehype-slug", { rehypePlugins: [...REHYPE, p.rehypeSlug] }],
  ["rehype-slug with a prefix", { rehypePlugins: [...REHYPE, [p.rehypeSlug, { prefix: "h-" }]] }],
  [
    "rehype-external-links",
    { rehypePlugins: [...REHYPE, [p.rehypeExternalLinks, { target: "_blank" }]], linkSafety: { enabled: false } },
  ],
  [
    "everything",
    {
      remarkPlugins: [remarkGfm, p.remarkBreaks, p.remarkMath],
      rehypePlugins: [
        defaultRehypePlugins.raw,
        defaultRehypePlugins.sanitize,
        defaultRehypePlugins.harden,
        p.rehypeSlug,
        p.rehypeKatex,
        [p.rehypeExternalLinks, { rel: "noopener", protocols: ["https"] }],
      ],
      linkSafety: { enabled: false },
    },
  ],
];

async function render(component: unknown, source: string, props: Record<string, unknown>) {
  const root = mount(component);
  const html = await root.render(source, props);
  await root.unmount();
  return html;
}

async function check(name: string, props: Record<string, unknown>) {
  const heard: string[] = [];
  setFallbackListener((reason) => heard.push(reason));
  const before = blockRenders.rill;
  try {
    for (const source of DOCUMENTS) {
      const expected = await render(Streamdown, source, props);
      assert.equal(await render(RillStreamdown, source, props), expected, `${name}: ${source.slice(0, 40)}`);
    }
  } finally {
    setFallbackListener(null);
  }
  assert.ok(blockRenders.rill > before, `${name}: rill rendered`);
  // Raw HTML the HTML5 parser must see is the only fallback.
  assert.deepEqual([...new Set(heard)].filter((reason) => reason !== "raw-html"), [], name);
}

for (const [label, p] of [["as published", plain], ["minified", packed]] as const) {
  for (const [name, props] of cases(p)) {
    test(`${name} (${label}) renders through rill, as Streamdown does`, () => check(name, props));
  }
}

test("plugins.math with singleDollarTextMath renders through rill", () =>
  check("single dollar", { plugins: { math: createMathPlugin({ singleDollarTextMath: true }) } }));

test("plugins.math with remark-math in the lists too stays with Streamdown", async () => {
  const heard: string[] = [];
  setFallbackListener((reason, detail) => heard.push(`${reason}: ${detail}`));
  try {
    await render(RillStreamdown, "$$x$$", {
      plugins: { math },
      remarkPlugins: [...REMARK, plain.remarkMath],
      rehypePlugins: [...REHYPE, plain.rehypeKatex],
    });
  } finally {
    setFallbackListener(null);
  }
  assert.match(heard[0] ?? "", /^remarkPlugins: remarkMath with plugins.math/);
});

test("a plugin rill does not know is named, and its blocks render with Streamdown", async () => {
  const heard: string[] = [];
  function rehypeOwn() {
    return () => {};
  }
  const props = { rehypePlugins: [...REHYPE, rehypeOwn, plain.rehypeSlug] };
  setFallbackListener((reason, detail) => heard.push(`${reason}: ${detail}`));
  try {
    const source = "# A\n\ntext";
    assert.equal(await render(RillStreamdown, source, props), await render(Streamdown, source, props));
  } finally {
    setFallbackListener(null);
  }
  assert.equal(heard[0], "rehypePlugins: rehypeOwn");
});

test("a plugin that only looks like a known one is checked, not trusted", async () => {
  const heard: string[] = [];
  // Named as rehype-slug, but numbers headings its own way.
  function rehypeSlug() {
    return (tree: { children: { tagName?: string; properties?: Record<string, unknown> }[] }) => {
      for (const node of tree.children) if (node.tagName === "h1") node.properties!.id = "mine";
    };
  }
  const props = { rehypePlugins: [...REHYPE, rehypeSlug] };
  setFallbackListener((reason, detail) => heard.push(`${reason}: ${detail}`));
  try {
    const source = "# A";
    assert.equal(await render(RillStreamdown, source, props), await render(Streamdown, source, props));
  } finally {
    setFallbackListener(null);
  }
  assert.equal(heard[0], "rehypePlugins: rehypeSlug");
});
