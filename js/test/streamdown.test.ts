import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { toHtml } from "hast-util-to-html";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { defaultRehypePlugins, defaultRemarkPlugins } from "streamdown";
import { unified, type PluggableList } from "unified";

import { createRill, initRill, type Rill } from "../src/index.ts";
import { rillPlugins } from "../src/streamdown.ts";

const rill = await initRill(
  readFileSync(new URL("../wasm/rill_bg.wasm", import.meta.url))
);

// Each kind rill reports, plus the shapes that most often differ between parsers.
const DOCUMENTS = [
  "# Title *em* **strong** ~~gone~~ `code`",
  "- [x] done\n- [ ] open\n  1. nested\n  2. list\n\n> quote\n> - in a quote",
  "| a | b |\n|:--|--:|\n| 1 | **2** |\n| x | [y](https://example.com) |",
  "www.example.com/a_(b) and https://example.com?q=1. and a@b.co",
  "```ts\nconst x: number = 1;\n```\n\n    indented code",
  "***a** b*  _c__d_ **e*f** ~~~not~~~",
  "Inline $$x^2$$ and a block:\n\n$$\n\\int_0^1 x\\,dx\n$$",
  "<details><summary>raw</summary>\n\n**html** <b>bold</b>\n\n</details>",
  "[link](javascript:alert(1)) ![img](data:image/png;base64,AAAA) [ok](/a)",
  "日本語の**強調**です。中文**加粗**测试，~~删除~~线",
  "Setext\n===\n\nline  \nbreak\\\nand *unclosed",
  "a\uD800b\0c",
];

function processor(remark: PluggableList, rehype: PluggableList) {
  return unified()
    .use(remarkParse)
    .use(remark)
    .use(remarkRehype, { allowDangerousHtml: true })
    .use(rehype);
}

function html(remark: PluggableList, rehype: PluggableList, source: string) {
  const run = processor(remark, rehype);
  return toHtml(run.runSync(run.parse(source), source) as never);
}

const streamdown = {
  remark: Object.values(defaultRemarkPlugins),
  rehype: Object.values(defaultRehypePlugins),
};

function sameAsStreamdown(engine: Rill, extra: object = {}) {
  const plugins = rillPlugins({ rill: engine, ...extra });
  for (const source of DOCUMENTS) {
    assert.equal(
      html(plugins.remarkPlugins, plugins.rehypePlugins, source),
      html(streamdown.remark, streamdown.rehype, source),
      source
    );
  }
}

test("rill's plugins render what Streamdown's do", () => {
  const kinds = new Set<string>();
  sameAsStreamdown(rill, {
    onRender: (kind: string) => kinds.add(kind),
  });
  assert.deepEqual([...kinds].sort(), ["complete", "math", "raw"]);
});

test("a document rill fails on renders with Streamdown's plugins", () => {
  const errors: unknown[] = [];
  const failing = createRill((): never => {
    throw new Error("boom");
  });
  sameAsStreamdown(failing, { onError: (error: unknown) => errors.push(error) });
  assert.equal(errors.length, DOCUMENTS.length);
});

test("your own plugins run after the pipeline", () => {
  const mark = () => (tree: { children: unknown[] }) => {
    tree.children.push({ type: "text", value: "!" });
  };
  const plugins = rillPlugins({ rill, after: { rehype: [mark] } });
  assert.equal(
    html(plugins.remarkPlugins, plugins.rehypePlugins, "*a*"),
    "<p><em>a</em></p>!"
  );
});

test("your own rehype chain needs its math tail", () => {
  assert.throws(
    () => rillPlugins({ rill, rehypePlugins: streamdown.rehype }),
    /mathTail/
  );
});
