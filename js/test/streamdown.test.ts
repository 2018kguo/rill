import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { cjk } from "@streamdown/cjk";
import { math } from "@streamdown/math";
import { toHtml } from "hast-util-to-html";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  defaultRehypePlugins,
  defaultRemarkPlugins,
  Streamdown,
  type PluginConfig,
} from "streamdown";
import { unified, type PluggableList } from "unified";

import { createRill, initRill, type Rill } from "../src/index.ts";
import { rillPlugins, type RenderKind } from "../src/plugins.ts";

const rill = await initRill(
  readFileSync(new URL("../wasm/rill_bg.wasm", import.meta.url))
);

// Each kind rill reports, plus the shapes that most often differ between parsers.
const DOCUMENTS = [
  "# Title *em* **strong** ~~gone~~ `code`",
  "- [x] done\n- [ ] open\n  1. nested\n  2. list\n\n> quote\n> - in a quote",
  "| a | b |\n|:--|--:|\n| 1 | **2** |\n| x | [y](https://example.com) |",
  "www.example.com/a_(b) and https://example.com?q=1. and a@b.co, __c@d.co",
  '```ts title="x.ts" {2}\nconst x: number = 1;\n```\n\n    indented code',
  "***a** b*  _c__d_ **e*f** ~~~not~~~",
  "Inline $$x^2$$ and a block:\n\n$$\n\\int_0^1 x\\,dx\n$$",
  // Math with what sanitize and harden change elsewhere in the tree.
  "$$\\frac{a}{b}$$ [bad](javascript:alert(1)) ![i](/x.png) <!-- c -->\n\n```math\na+b\n```",
  "<details><summary>raw</summary>\n\n**html** <b>bold</b>\n\n</details>",
  "[link](javascript:alert(1)) ![img](data:image/png;base64,AAAA) [ok](/a)",
  "[call](tel:+1-555-0100) [more](streamdown:incomplete-link) ![i](streamdown:incomplete-image)",
  'A note[^1] and <a id="x" name="y">anchors</a>.\n\n[^1]: The note.',
  "日本語の**強調**です。中文**加粗**测试，~~删除~~线，**「括号」**也可以。",
  "访问 https://example.com/a。然后 www.example.org，再见",
  "Setext\n===\n\nline  \nbreak\\\nand *unclosed",
  "a\uD800b\0c",
];

type Plugins = Pick<PluginConfig, "math" | "cjk">;
const CONFIGS: Record<string, Plugins> = {
  default: {},
  math: { math },
  cjk: { cjk },
  "math+cjk": { math, cjk },
};

/** What `<Streamdown plugins={plugins}>` runs around its two plugin props
 * (index.tsx `mergedRemarkPlugins`, `mergedRehypePlugins`). */
function merged(
  plugins: Plugins,
  remark: PluggableList = Object.values(defaultRemarkPlugins),
  rehype: PluggableList = Object.values(defaultRehypePlugins)
) {
  return {
    remark: [
      ...(plugins.cjk?.remarkPluginsBefore ?? []),
      ...remark,
      ...(plugins.cjk?.remarkPluginsAfter ?? []),
      ...(plugins.math ? [plugins.math.remarkPlugin] : []),
    ],
    rehype: [...rehype, ...(plugins.math ? [plugins.math.rehypePlugin] : [])],
  };
}

function html({ remark, rehype }: ReturnType<typeof merged>, source: string) {
  const run = unified()
    .use(remarkParse)
    .use(remark)
    .use(remarkRehype, { allowDangerousHtml: true })
    .use(rehype);
  return toHtml(run.runSync(run.parse(source), source) as never);
}

function sameAsStreamdown(
  engine: Rill,
  plugins: Plugins,
  extra: Partial<Parameters<typeof rillPlugins>[0]> = {}
) {
  const own = rillPlugins({ rill: engine, plugins, ...extra });
  for (const source of DOCUMENTS) {
    assert.equal(
      html(merged(plugins, own.remarkPlugins, own.rehypePlugins), source),
      html(merged(plugins), source),
      source
    );
  }
}

for (const [name, plugins] of Object.entries(CONFIGS)) {
  test(`rill's plugins render what Streamdown's do (${name})`, () => {
    const kinds = new Set<string>();
    sameAsStreamdown(rill, plugins, {
      onRender: (kind: RenderKind) => kinds.add(kind),
    });
    const expected = plugins.math
      ? ["complete", "math", "raw"]
      : ["complete", "raw"];
    assert.deepEqual([...kinds].sort(), expected);
  });

  test(`<Streamdown> renders the same page with rill's plugins (${name})`, () => {
    // Counted, so plugins rill never ran in cannot pass as "the same page".
    let rendered = 0;
    const own = rillPlugins({
      rill,
      plugins,
      onRender: (kind) => {
        if (kind !== "javascript") {
          rendered += 1;
        }
      },
    });
    for (const source of DOCUMENTS) {
      const page = (props: object) =>
        renderToStaticMarkup(
          createElement(
            Streamdown,
            { mode: "static", plugins, ...props },
            source
          )
        );
      assert.equal(page(own), page({}), source);
    }
    assert.ok(rendered >= DOCUMENTS.length - 1, `rill rendered ${rendered}`);
  });
}

test("plugins rill was not told about render in JavaScript, the same", () => {
  const kinds = new Set<string>();
  const own = rillPlugins({ rill, onRender: (kind) => kinds.add(kind) });
  for (const source of DOCUMENTS) {
    const page = (props: object) =>
      renderToStaticMarkup(
        createElement(
          Streamdown,
          { mode: "static", plugins: { math, cjk }, ...props },
          source
        )
      );
    assert.equal(page(own), page({}), source);
  }
  assert.deepEqual([...kinds], ["javascript"]);
});

test("a document rill fails on renders with Streamdown's plugins", () => {
  const errors: unknown[] = [];
  const failing = createRill((): never => {
    throw new Error("boom");
  });
  sameAsStreamdown(
    failing,
    { math, cjk },
    {
      onError: (error: unknown) => errors.push(error),
    }
  );
  assert.equal(errors.length, DOCUMENTS.length);
});

test("a replaced remarkPlugins still runs the whole rehype chain", () => {
  const { rehypePlugins } = rillPlugins({ rill });
  for (const source of DOCUMENTS) {
    assert.equal(
      html(merged({}, undefined, rehypePlugins), source),
      html(merged({}), source),
      source
    );
  }
});

test("without rehype-raw, raw HTML is text, as Streamdown shows it", () => {
  const rehype = [defaultRehypePlugins.sanitize, defaultRehypePlugins.harden];
  const kinds = new Set<string>();
  const own = rillPlugins({
    rill,
    rehypePlugins: rehype,
    onRender: (kind) => kinds.add(kind),
  });
  for (const source of DOCUMENTS) {
    const page = (props: object) =>
      renderToStaticMarkup(
        createElement(
          Streamdown,
          { mode: "static", rehypePlugins: rehype, ...props },
          source
        )
      );
    assert.equal(page(own), page({}), source);
  }
  assert.deepEqual([...kinds], ["javascript"]);
});

test("documents over maxSourceLength render in JavaScript", () => {
  const kinds: string[] = [];
  const own = rillPlugins({
    rill,
    maxSourceLength: 4,
    onRender: (kind) => kinds.push(kind),
  });
  const source = "**long** enough";
  assert.equal(
    html(merged({}, own.remarkPlugins, own.rehypePlugins), source),
    html(merged({}), source)
  );
  assert.deepEqual(kinds, ["javascript"]);
});

test("your own rehype plugins run after the pipeline", () => {
  const mark = () => (tree: { children: unknown[] }) => {
    tree.children.push({ type: "text", value: "!" });
  };
  const own = rillPlugins({ rill, after: [mark] });
  assert.equal(
    html(merged({}, own.remarkPlugins, own.rehypePlugins), "*a*"),
    "<p><em>a</em></p>!"
  );
});

test("a remark plugin <Streamdown> adds sends documents to JavaScript", () => {
  const kinds = new Set<string>();
  const own = rillPlugins({ rill, onRender: (kind) => kinds.add(kind) });
  const source = "Mail a@b.co or see https://example.com";
  const page = (props: object) =>
    renderToStaticMarkup(
      createElement(
        Streamdown,
        { mode: "static", disableAutolinkProtocols: ["mailto"], ...props },
        source
      )
    );
  assert.equal(page(own), page({}));
  assert.ok(!page({}).includes("mailto:"));
  assert.deepEqual([...kinds], ["javascript"]);
});
