# rill

**Render AI chat Markdown 10× faster, with the exact same output.**

rill is a drop-in engine for [Streamdown](https://github.com/vercel/streamdown), the
Markdown renderer behind a lot of AI chat UIs. It's written in Rust, runs in the browser
through WebAssembly, and produces the same result as Streamdown down to every node and
character position. You get the same page, just much sooner.

![rill and Streamdown rendering the same answer side by side as it streams in](docs/demo.gif)

## Use it

```sh
npm install @2018kguo/rill
```

```tsx
import { Streamdown } from "streamdown";
import { useRill } from "@2018kguo/rill/react";

export function Message({ markdown }: { markdown: string }) {
  return <Streamdown {...useRill()}>{markdown}</Streamdown>;
}
```

That's the whole change. Until the WebAssembly arrives (and during server rendering)
`useRill()` returns `null` and Streamdown renders as it always has; then rill takes over,
with the same output, so nothing on the page moves. If rill ever can't take a document,
that one renders with Streamdown's own plugins.

With Vite, keep the package out of dependency pre-bundling, which would move the
WebAssembly file away from the code that loads it:

```ts
// vite.config.ts
export default { optimizeDeps: { exclude: ["@2018kguo/rill"] } };
```

## Why

When a chatbot streams an answer, the screen updates many times a second. Each update
re-renders the whole answer so far, because a half-written `**bold` or table can change
meaning when the next few characters arrive. Long answers make every update slower,
and that cost lands right where users notice it: while the text is still coming in.

Streamdown does this well, but it's built from dozens of small JavaScript packages
(remark and rehype plugins) that each walk the document again. rill does the same work in
one compiled pass:

| 97 KB answer | Streamdown (JavaScript) | rill |
|---|---:|---:|
| Browser (WASM, per render) | ≈1× | 8–11× faster |
| Native (Rust) | 29.2 ms | 0.31 ms |

"Same output" is the whole point. Switching renderers usually means subtly different
lists, links or emphasis. rill is checked against Streamdown's own pipeline, so
switching changes nothing on the page.

## How it works

Rendering Markdown in Streamdown is a pipeline:

1. **Parse** the text into a syntax tree (remark, with GitHub tables and task lists,
   math, and better CJK handling).
2. **Convert** that tree into an HTML tree.
3. **Clean it up**: normalize raw HTML, strip anything unsafe, and block dangerous links.

rill reimplements all three steps in Rust, with each step ported from the JavaScript
package it replaces. Two pieces stay in JavaScript, and rill tells you when they're
needed: KaTeX for math, and the HTML5 parser for documents containing raw HTML. Plain
Markdown, which is most chat output, never touches JavaScript.

## How it was built

The parser starts from [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark),
a fast Rust Markdown parser. Its rules were then changed, one at a time, until they agree
with micromark, the parser Streamdown actually uses, on every input we could generate.

The method:

- **Diff against the real thing.** A harness runs Streamdown's JavaScript pipeline and
  rill on the same input and compares the trees, positions included.
- **Try every small input.** Fuzzing random documents finds bugs, but the failures are
  long and noisy. Instead, the harness enumerates *every* string up to a fixed length
  over a small set of characters like `*`, `>`, `|` and `` ` `` plus newlines. Millions
  of documents run in seconds, and each mismatch arrives as its smallest possible
  example.
- **Fix the rule, not the case.** Each minimal example was traced back to micromark's
  source to find the rule it exposed. Examples: emphasis resolves before strikethrough
  when it appears first, and a list ending inside a blockquote ends after the `>`, not
  before. Each fix is that rule written down, and the sweeps then confirm it held
  everywhere.

Today every exhaustive sweep matches exactly, across emphasis, lists and quotes, tables,
links, math and URLs, and random fuzzing over a wider vocabulary matches 99.9%. The
known differences are listed under [Conformance](#conformance).

## Try it

```sh
npm install
npm run demo          # http://localhost:4173: edit Markdown, compare both, replay a stream
```

The demo needs Rust with the `wasm32-unknown-unknown` target and
`cargo install wasm-bindgen-cli --version 0.2.129`.

---

## Reference

### Rust

```rust
use rill::{Kind, Options};

let (kind, hast_json) = rill::render("# Hello *world*", &Options::default())?;
assert_eq!(kind, Kind::Complete);

let mdast_json = rill::parse_json("- [x] done")?;
```

`rill::mdast::parse_arena` returns the syntax tree itself, an arena that borrows the
source text instead of copying it.

### JavaScript

`@2018kguo/rill` ships the WebAssembly prebuilt, so you don't need Rust to use it.

- `@2018kguo/rill/react`: `useRill()`, the drop-in above.
- `@2018kguo/rill/streamdown`: `rillPlugins({ rill })` returns the `remarkPlugins` and
  `rehypePlugins` to hand `<Streamdown>`, for when you manage loading yourself (memoize
  it). It takes `after` for rehype plugins of your own (they run on the finished tree,
  like any plugin after Streamdown's), `maxSourceLength` (longer documents render in
  JavaScript; 128K UTF-16 units by default, since WebAssembly memory grows to fit the
  largest document and never shrinks), and `onRender` and `onError` callbacks.
- `@2018kguo/rill`: `loadRill()` fetches the WebAssembly once; `initRill(bytes)` instantiates
  it from bytes (Node, tests); `rill.render(markdown)` returns `{ kind, tree }` directly.

```ts
import { loadRill } from "@2018kguo/rill";
import { rillPlugins } from "@2018kguo/rill/streamdown";

const plugins = rillPlugins({ rill: await loadRill(), after: [rehypeSlug] });
// <Streamdown {...plugins}>{markdown}</Streamdown>
```

Three things rill can't do for you:

- **Combine its plugins with your own `remarkPlugins` or `rehypePlugins` props.** Pass
  rill's lists as they are and add rehype plugins through `after`. (A replaced
  `remarkPlugins` still renders safely, all in JavaScript.)
- **Remark plugins.** rill parses in Rust, so there is no syntax tree in JavaScript for
  them to change. Write a Rust plugin (below) or work on the HTML tree with `after`.
- **`remarkRehypeOptions`.** rill renders Streamdown's setting (`allowDangerousHtml`).

The WebAssembly loads with `new URL("../wasm/rill_bg.wasm", import.meta.url)`, which Vite,
webpack 5, Next.js, Rollup and esbuild all resolve to a served asset.

### What runs where

| Stage | Package ported | Runs in |
|---|---|---|
| Parse | remark-parse 11, remark-gfm 4, remark-math 6 (`singleDollarTextMath: false`), remark-cjk-friendly 1.2 (and its strikethrough) | Rust |
| MDAST → HAST | remark-rehype 11 (`allowDangerousHtml`) | Rust |
| Raw HTML | rehype-raw 7 | Rust without raw HTML, else JavaScript |
| Math | rehype-katex 7 | JavaScript |
| Sanitize | rehype-sanitize 6, default schema | Rust |
| Harden | rehype-harden 1.1.8 with Streamdown's options | Rust |

`render` reports how far it got:

- `complete`: the whole chain ran, and the tree is final.
- `raw`: the document has raw HTML (or a link inside a link), which needs parse5. Run
  rehype-raw, rehype-katex, rehype-sanitize and rehype-harden on the tree.
- `math`: Streamdown runs KaTeX before sanitizing, so rehype-katex, rehype-sanitize and
  rehype-harden still run on the tree.

`conformance/pipeline.mjs` has these tails ready to use.

### Options and plugins

`Options` defaults to Streamdown's setup:

- `harden`: link and image block policies, `Indicator` (default), `TextOnly` or `Remove`.
- `sanitize.code_classes`: extra `className`s `<code>` may keep.
- `katex_after_sanitize`: KaTeX runs after sanitizing instead of before it.
- `plugins`: your transforms.

A plugin implements up to three hooks, at the points where a remark or rehype plugin
would run:

```rust
impl rill::Plugin for HeadingIds {
    // After parsing, on the syntax tree (remark plugins run here).
    fn mdast(&self, tree: &mut rill::mdast::MTree<'_>) -> Result<(), rill::Error> { Ok(()) }
    // On the HTML tree before sanitizing.
    fn hast_unsanitized(&self, tree: &mut rill::hast::HTree<'_>, root: u32, cx: &rill::Context<'_>) {}
    // On the final HTML tree (rehype plugins after the chain).
    fn hast(&self, tree: &mut rill::hast::HTree<'_>, root: u32, cx: &rill::Context<'_>) {}
}
```

`crates/rill/tests/api.rs` has a complete one that adds heading ids. Any JavaScript
rehype plugin also works unchanged on rill's output (`after` above), so a Rust plugin is
only worth it for work that has to happen earlier, such as on the syntax tree.

To ship Rust plugins, build your own WebAssembly with one export that calls
`rill::wasm::render` (the `wasm` feature) and hand it to the same JavaScript package:

```rust
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = renderHast)]
pub fn render_hast(source: &str) -> Result<rill::wasm::RenderedHast, JsError> {
    rill::wasm::render(source, &my_options())
}
```

```ts
import init, { renderHast } from "./pkg/my_markdown.js";
import { createRill } from "@2018kguo/rill/core"; // core: no prebuilt WebAssembly
import { rillPlugins } from "@2018kguo/rill/streamdown";

await init();
const plugins = rillPlugins({
  rill: createRill(renderHast),
  // The JavaScript pipeline your options mirror: the fallback, and what still runs on
  // raw HTML (all of `rehypePlugins`) and math (`mathTail`: KaTeX and what follows it).
  remarkPlugins,
  rehypePlugins,
  mathTail,
});
```

Extra arguments on your export pass through: `createRill(renderHast)` gives
`rill.render(source, ...args)`, and `rillPlugins({ args })` (required when your export
takes any) supplies them per instance.

### Conformance

```sh
cargo build --release --example stdio
node conformance/diff.mjs                                     # the examples
node conformance/diff.mjs --alphabet '~*_[]a `' --length 5    # every string over these
node conformance/diff.mjs --pieces '["- ","> ","```","\n"]' --length 6
node conformance/diff.mjs --fuzz 3000 --seed 7 --minimize     # random, with minimal witnesses
node conformance/probe.mjs '"> - a\n>"'                       # both trees side by side
```

The harness compares the MDAST (minus `data`) and the final HAST, positions included,
against the latest releases of the packages above.

Known differences, all of them position-only or in rare shapes:

- Tabs in list-item indentation count as one column where micromark expands them.
- A block that starts on a lazy line after an empty container, such as indented code
  after a bare `>`, can end one line off.
- micromark miscounts the text before a NUL byte; rill reports what U+FFFD would give.

### Benchmarks

```sh
cargo run --release -p rill --example bench    # rill, native
node conformance/bench.mjs                     # the JavaScript pipeline, same document
```

### Layout

- `crates/rill-cmark`: the pulldown-cmark 0.13.4 fork, with every change from upstream
  marked `rill:`.
- `crates/rill`: `mdast.rs` (the syntax tree), `hast.rs` (HTML tree, sanitize, harden,
  plugins), `autolink.rs`, and `wasm` (the boundary the JavaScript package reads).
- `js/`: the `@2018kguo/rill` npm package (loader, opcode decoder, Streamdown plugins,
  React hook) and its test against Streamdown's own pipeline.
- `conformance/`: the differential harness, the probe and the benchmark.
- `demo/`: the page in the GIF.

## License

MIT. `crates/rill-cmark` is derived from pulldown-cmark (MIT, Copyright 2015 Google Inc.).
