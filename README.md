# rill

**Streamdown, without the wait. Same DOM, byte for byte.**

rill is a drop-in for [Streamdown](https://github.com/vercel/streamdown) 2.7.1, the
Markdown component behind a lot of AI chat UIs. Change one import and the same component
renders the same page, down to every class, attribute and text node, with most of the
work moved into Rust and WebAssembly. On a throttled phone, a long chat opens 4.7× sooner
and answers stream at full frame rate instead of stuttering.

[![Streamdown and rill streaming the Rust Book's ownership chapter side by side at 10× CPU throttle: 28 fps vs 65 fps](docs/rill-0.2.jpg)](https://github.com/2018kguo/rill/releases/tag/v0.2.0)

*The launch film (70 s, real recordings): [watch it on the release page](https://github.com/2018kguo/rill/releases/tag/v0.2.0).*

## Use it

```sh
npm install @2018kguo/rill
```

Change one import:

```diff
- import { Streamdown } from "streamdown";
+ import { Streamdown } from "@2018kguo/rill/streamdown";
```

`@2018kguo/rill/streamdown` exports everything `streamdown` 2.7.1 exports, so the rest of
your imports (`Block`, `CodeBlock`, `defaultComponents`, types...) keep working from the
new path, including a `BlockComponent` of your own that wraps `Block`: import `Block`
from here too and it renders with rill. The component takes the same props and renders
the same DOM, down to every class, attribute and text node, with the same behavior
(copy and download buttons, table menus, the link safety modal, the caret, server
rendering and hydration, `components` overrides, `plugins`).

What changes is the work behind it. Streamdown is still the component: its root, its
contexts and its memoization are its own, and its blocks are the ones it would split
(the same splitter, on the same `marked`). rill renders each block in Rust: static
markup becomes one React element with its HTML, and React only builds the interactive
parts (code block shells, tables, images, link buttons) from Streamdown's own exported
components. A code block's copy and download buttons start as their markup and become
Streamdown's own buttons when first pressed (which then get that click). With
`@streamdown/code`, code is tokenized by
[rill-highlight](https://github.com/2018kguo/rill-highlight) (`@2018kguo/rill-highlight`,
Shiki's tokenizer ported to Rust, the same tokens) in a Web Worker, which `setHighlightWasm` (or
`preloadHighlighting()`) starts with its WebAssembly and themes before the first code
block; finished code blocks ask for their tokens as they render, and the replies of a
batch land in one React render.

While an answer streams, each update costs what the block being written costs, not
the whole answer: Streamdown's streaming repair (remend) runs in Rust on the text after
a prefix it has settled, the block splitter lexes only the blocks that can still change,
and a block that link definitions made one (`[1]: https://...` citations keep the whole
answer in one block in Streamdown) renders as its pieces, of which only the last changes.
A message that has not streamed (a conversation opened from history) renders its blocks
from one component instead of one per block, and what it computes from its text alone is
cached by content, so mounting it again is cheap. `mode="static"` renders the whole text
in one pass, as Streamdown's static mode does, through rill too.

Anything rill doesn't reproduce exactly renders with Streamdown's own `Block`, the same
way: raw HTML that needs the HTML5 parser (rill handles comments, phrasing tags, `<img>`
and `<br>` with or without attributes, and tags sanitize removes around Markdown, such as
custom elements or the Rust Book's `<Listing>`), `animated`, `allowedTags`, custom
`remarkPlugins`/`rehypePlugins` (bring them as a [custom build](#custom-builds) instead),
`urlTransform` and the other Markdown filters, `fallbackComponent`, nesting deeper than
256 levels (quotes, lists, emphasis, raw tags), `mode="static"` with `dir`, and every
block until the WebAssembly has loaded (and on the server without `initRill`).

The other tiers stay:

- `@2018kguo/rill/react`: `useRill(plugins?)` returns `remarkPlugins`/`rehypePlugins` for
  Streamdown's own component (`<Streamdown plugins={plugins} {...useRill(plugins)}>`):
  rill parses, Streamdown's React renders.
- `@2018kguo/rill/plugins` (also exported from `/streamdown`): `rillPlugins`, the same
  plugins when you load rill yourself.

### Setup per bundler

The WebAssembly and the highlighting worker load by themselves, from URLs the bundler
emits (`new URL(..., import.meta.url)`): no files to copy, no config.

- **Next.js** (App Router; Turbopack and webpack, `next dev` and `next build`): nothing
  to configure. Import it in client components or render it straight from a server
  component, as you would Streamdown: the server renders it with Streamdown's own
  pipeline and the page hydrates without a warning
  ([examples/nextjs](examples/nextjs)).
- **Vite 8**, and frameworks on it such as React Router (SSR included): nothing to
  configure ([examples/vite](examples/vite), [examples/react-router](examples/react-router)).
- **Vite 7 and older**: one plugin. Vite 7 pre-bundles dependencies in a way that loses
  those URLs, and its build needs ES module workers:

  ```ts
  // vite.config.ts
  import { rill } from "@2018kguo/rill/vite";
  export default defineConfig({ plugins: [react(), rill()] });
  ```

- **bun build**: serve `@2018kguo/rill-highlight/dist/rill_highlight.wasm` and pass its
  URL to `setHighlightWasm` (bun doesn't emit it).

CI builds each example in each of these modes and checks it in Chromium: server-rendered
where the app renders on the server, no console error or hydration warning, both
WebAssembly modules served as `application/wasm`, code highlighted by rill-highlight in
its worker, and a streamed answer rendered entirely on rill's fast path
(`examples/run.sh`, `examples/smoke.mjs`). Other bundlers that emit
`new URL(..., import.meta.url)` assets and module workers (webpack 5 on its own, Rollup,
Parcel) should work the same way, but CI doesn't check them.

## Why

When a chatbot streams an answer, the screen updates many times a second, and each
update re-renders the answer so far, because a half-written `**bold` or table can change
meaning when the next characters arrive. Opening a chat renders every past answer at once.
On a phone, that work is what users feel: stutter while text arrives, a blank screen
while a conversation opens.

rill keeps Streamdown's component and does its work in one compiled pass. Measured in
Chromium with production React, a 412×915 phone screen and 10× CPU throttling (≈ a budget
Android phone); every number is a median of trace-based runs, Streamdown 2.7.1 vs rill
0.2.0, with byte-identical final HTML:

| Scenario | Finished sooner | Less main-thread JS + GC | Heap after |
|---|---:|---:|---:|
| Open a chat of 18 long answers (231 KB of text, code, tables) | 4.7× | 6.8× | 40.9 → 13.4 MB |
| Open a code-heavy chat | 7.5× | 10.8× | |
| Stream the 118-element periodic table as one table | 4.1× | 15.7× | |
| Stream the Rust Book's ownership chapter (56 KB) | 5.5× | 13.3× | |

Layout and paint are the same on both sides (it's the same DOM), so "finished sooner" is
bounded by what the browser itself spends; "less JS + GC" is rill's own share. Streaming at
a realistic 2,000–3,000 characters a second, recorded live at 10× throttle, Streamdown
averages 15–28 fps with 95th-percentile frames of 155–185 ms; rill averages 60–65 fps
with 95th-percentile frames of 26–36 ms.

### Real app: Vercel's AI chatbot

The numbers above come from a page that only renders Markdown. In a real app, React
also re-renders the app around each answer. [vercel/ai-chatbot](https://github.com/vercel/ai-chatbot)
(Next.js 16, React 19, Streamdown with its `cjk`, `code`, `math` and `mermaid` plugins),
built for production as it is and with rill's import (one line in each of the two files
that render Markdown, no config), on the same phone screen, with the template's mock
model and no API keys. Medians of 5 interleaved trace-based runs, with 95% bootstrap
intervals on the ratio; Streamdown 2.7.1 → rill:

| Scenario | CPU | Settled | Main-thread JS + GC | Long-frame blocking | Heap after |
|---|---:|---:|---:|---:|---:|
| Open a chat of 18 long answers (239 KB) | 4× | 2,991 → 1,942 ms (1.54× [1.53–2.66]) | 2,735 → 1,535 ms (1.78×) | 2,150 → 1,221 ms | 49.5 → 24.2 MB |
| | 10× | 7,964 → 2,863 ms (2.78× [2.71–2.84]) | 7,338 → 2,214 ms (3.31× [3.23–3.39]) | 6,820 → 2,135 ms | 48.9 → 22.0 MB |
| Stream an 8 KB answer (code, tables, a list) at 2,400 chars/s | 4× | 4,512 → 4,582 ms (stream-bound) | 3,404 → 2,239 ms (1.52× [1.41–1.57]) | 497 → 4 ms | 27.5 → 47.3 MB |
| | 10× | 10,056 → 6,427 ms (1.56× [1.41–1.64]) | 9,360 → 5,638 ms (1.66× [1.45–1.71]) | 8,401 → 4,070 ms | 26.7 → 19.8 MB |

Opening the chat, the time from the messages' response to settled falls 1.70× at 4× and
3.88× [3.78–3.99] at 10× (2,591 → 1,527 ms, 6,913 → 1,782 ms); the rest is the app
loading, the same on both sides. At 4× the stream arrives no faster than it is sent, so
wall time is the stream's; what changes is that the main thread stops blocking (long
frames 497 → 4 ms, dropped frames 32% → 21%). At 10× the template's own React work per
streamed chunk (the message list, the chat state) is most of what remains, which is why
the gains are smaller than on the Markdown-only page. Heap after the 4× stream is the one
number that got worse. The settled answers' HTML is byte-identical in every run of both
builds. The harness and the template patches are in `bench/ai-chatbot`.

The Markdown pipeline alone (remark, rehype, sanitize, harden, Streamdown's plugins) is a
smaller tier of the same story:

| 97 KB answer | Streamdown (JavaScript) | rill |
|---|---:|---:|
| Browser (WASM, per render) | ≈1× | 8–11× faster |
| Native (Rust) | 29.2 ms | 0.31 ms |

"Same output" is the whole point. Switching renderers usually means subtly different
lists, links or emphasis. rill is checked against Streamdown's own pipeline and its DOM,
so switching changes nothing on the page.

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
use rill::{Kind, Options, Syntax};

let rendered = rill::render("# Hello *world*", &Options::default())?;
assert_eq!(rendered.kind, Kind::Complete);

let mdast_json = rill::parse_json("- [x] done", Syntax::default())?;
```

`rill::mdast::parse_arena` returns the syntax tree itself, an arena that borrows the
source text instead of copying it.

### JavaScript

`@2018kguo/rill` ships the WebAssembly prebuilt, so you don't need Rust to use it.

- `@2018kguo/rill/streamdown`: `Streamdown` and `Block`, the drop-in above, plus everything
  `streamdown` exports. `setRillHighlight(false)` keeps Shiki for code;
  `setHighlightWasm(url)` points at `rill_highlight.wasm` (from
  `@2018kguo/rill-highlight/dist/`) where a bundler doesn't place it by itself, as bun
  doesn't (Vite and webpack do), and starts the highlighting worker;
  `preloadHighlighting()` starts it without one.
- `@2018kguo/rill/react`: `useRill(plugins?)`, plugins for Streamdown's own component.
- `@2018kguo/rill/engine`: `defineEngine`, a [custom build](#custom-builds) for
  `<Streamdown engine>`; `rillEngine` is the prebuilt one, defined the same way.
- `@2018kguo/rill/vite`: `rill()`, the Vite plugin for Vite 7 and older.
- `@2018kguo/rill/conformance`: `checkEngine`, a custom build against its JavaScript
  plugins, DOM against DOM.
- `@2018kguo/rill/plugins`: `rillPlugins({ rill, plugins })` returns the `remarkPlugins`
  and `rehypePlugins` to hand `<Streamdown>`, for when you manage loading yourself (memoize
  it). `plugins` is the `plugins` prop you give `<Streamdown>` (its `math` and `cjk`);
  when the two disagree, documents render in JavaScript, with the same output. With
  `plugins.math`, rill renders each formula with its KaTeX once and caches it. It takes
  `after` for rehype plugins of your own (they run on the finished tree,
  like any plugin after Streamdown's), `maxSourceLength` (longer documents render in
  JavaScript; 128K UTF-16 units by default, since WebAssembly memory grows to fit the
  largest document and never shrinks), and `onRender` and `onError` callbacks.
- `@2018kguo/rill`: `loadRill()` fetches the WebAssembly once; `initRill(bytes)` instantiates
  it from bytes (Node, tests); `rill.render(markdown, { math, cjk })` returns `{ kind, tree }`
  directly.

```ts
import { loadRill } from "@2018kguo/rill";
import { rillPlugins } from "@2018kguo/rill/plugins";

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
- **`allowedTags`.** Streamdown widens its sanitize schema only with its own default
  `rehypePlugins`, so with rill's the extra tags are removed.

Without rehype-raw in `rehypePlugins` (Streamdown then shows raw HTML as text), and with
`disableAutolinkProtocols` or any other remark plugin `<Streamdown>` adds, every document
renders in JavaScript.

The WebAssembly loads with `new URL("../wasm/rill_bg.wasm", import.meta.url)` as soon as
the browser evaluates `@2018kguo/rill/streamdown`; see [Setup per bundler](#setup-per-bundler)
for where that resolves.

### What runs where

| Stage | Package ported | Runs in |
|---|---|---|
| Parse | remark-parse 11, remark-gfm 4, Streamdown's remarkCodeMeta; `plugins.math`: remark-math 6 (`singleDollarTextMath: false`); `plugins.cjk`: remark-cjk-friendly 2.3 (and its strikethrough) and its autolink boundary | Rust |
| MDAST → HAST | remark-rehype 11 (`allowDangerousHtml`) | Rust |
| Raw HTML | rehype-raw 7 | Rust without raw HTML, else JavaScript |
| Sanitize | rehype-sanitize 6, Streamdown 2.7.1's schema | Rust |
| Harden | rehype-harden 1.1.8 with Streamdown's options | Rust |
| Math | `plugins.math`: rehype-katex 7 | JavaScript |

`render` reports how far it got:

- `complete`: the whole chain ran, and the tree is final.
- `raw`: the document has raw HTML (or a link inside a link), which needs parse5. Run
  rehype-raw, rehype-sanitize and rehype-harden (and with math, rehype-katex) on the tree.
- `math` (with `math` on): everything is final but the math elements, for rehype-katex,
  which Streamdown runs last.

`conformance/pipeline.mjs` has these tails ready to use.

### Options and plugins

`Options` defaults to Streamdown's setup with no `plugins` prop:

- `math`, `cjk`: Streamdown's `plugins.math` (`@streamdown/math`) and `plugins.cjk`
  (`@streamdown/cjk`).
- `legacy_cjk`, `code_meta: false`, `sanitize.schema: Schema::Default`: Streamdown 1.x's
  default plugins, which pages passing their own lists often still run
  (remark-cjk-friendly 1.2.3 after remark-gfm, no remarkCodeMeta, hast-util-sanitize's
  `defaultSchema`; `diff.mjs --config classic` checks them).
- `harden`: link and image block policies, `Indicator` (default), `TextOnly` or `Remove`.
- `sanitize.code_classes`: extra `className`s `<code>` may keep.
- `plugins`: your transforms.

A plugin implements up to three hooks, at the points where a remark or rehype plugin
would run:

```rust
impl rill::Plugin for HeadingIds {
    // After parsing, on the syntax tree (remark plugins run here). Err(Unsupported)
    // leaves the document to the JavaScript plugins.
    fn mdast(&self, tree: &mut rill::mdast::Tree<'_>) -> Result<(), rill::Error> { Ok(()) }
    // On the HTML tree before sanitizing.
    fn hast_unsanitized(&self, tree: &mut rill::hast::Tree<'_>, root: u32, cx: &rill::Context<'_>) {}
    // On the final HTML tree (rehype plugins after the chain), before KaTeX.
    fn hast(&self, tree: &mut rill::hast::Tree<'_>, root: u32, cx: &rill::Context<'_>) {}
}
```

`crates/rill/tests/api.rs` has a complete one that adds heading ids. Any JavaScript
rehype plugin also works unchanged on rill's output (`after` above), so a Rust plugin is
only worth it for work that has to happen earlier, such as on the syntax tree.

### Custom builds

A custom build is your plugins and options in Rust, compiled to WebAssembly, rendered by
the drop-in exactly as the prebuilt build is: the same markup path, the same speed. You
define the options once, for every entry point, and the build's engine carries the
JavaScript plugins it reproduces, which render whatever rill leaves to Streamdown:

```rust
// lib.rs of a cdylib crate depending on rill with the `wasm` feature
rill::wasm_exports!(|base: Option<String>, hashtags: bool| my_options(base, hashtags));
```

```ts
import { defineEngine } from "@2018kguo/rill/engine";

export const myEngine = defineEngine({
  glue: () => import("./pkg/my_build.js"), // wasm-bindgen --target web
  wasm: () => new URL("./pkg/my_build_bg.wasm", import.meta.url),
  // The JavaScript plugins your options reproduce, per set of arguments.
  plugins: (base: string | null, hashtags: boolean) => ({ remarkPlugins, rehypePlugins }),
  katex: rehypeKatex, // only if your options turn math on themselves
});

<Streamdown engine={myEngine} engineArgs={[base, true]}>{markdown}</Streamdown>;
```

`engineArgs` compare by value (a new array each render is fine), and with an engine
`remarkPlugins`/`rehypePlugins` are a type error: the engine's are the ones. Until the
WebAssembly loads, and on the server unless you `await myEngine.init(bytes)`, Streamdown
renders with the engine's plugins, so hydration matches either way. A plugin that says
it is `piecewise` (its output for a block depends on that block alone) lets rill render a
long block that link definitions made one as pieces while it streams; one that counts
across the whole text (positions, markers) leaves such a block whole, as Streamdown does.

`checkEngine` (`@2018kguo/rill/conformance`) renders documents with Streamdown and your
engine's plugins and with your build, streamed and static, and compares the DOMs, in
happy-dom, jsdom or a browser:

```ts
await myEngine.init(readFileSync("pkg/my_build_bg.wasm"));
const report = await checkEngine({ engine: myEngine, args: ["/u", true], documents, stream: 20 });
assert.equal(report.identical, report.renders);
```

`examples/custom-build` is a complete one (math, three plugins, two arguments), which
`js/test/engine.test.ts` and the DOM harness (`--variant engine`) check.

The lower tier takes a build too: `myEngine.rill(...args)` is the `Rill` for
`rillPlugins({ rill })` and `rill.render(source)`.

### Conformance

```sh
cargo build --release --example stdio
node conformance/diff.mjs                                     # the examples
node conformance/diff.mjs --config math+cjk                   # with Streamdown's plugins
node conformance/diff.mjs --alphabet '~*_[]a `' --length 5    # every string over these
node conformance/diff.mjs --pieces '["- ","> ","```","\n"]' --length 6
node conformance/diff.mjs --fuzz 3000 --seed 7 --minimize     # random, with minimal witnesses
node conformance/probe.mjs '"> - a\n>"'                       # both trees side by side
```

The harness compares the MDAST (minus `data`) and the final HAST, positions included,
against Streamdown 2.7.1's pipeline, built from the published `streamdown`,
`@streamdown/math` and `@streamdown/cjk` packages, for each `plugins` configuration
(`default`, `math`, `cjk`, `math+cjk`).

Known differences, all of them position-only or in rare shapes:

- Tabs in list-item indentation count as one column where micromark expands them.
- A block that starts on a lazy line after an empty container, such as indented code
  after a bare `>`, can end one line off.
- micromark miscounts the text before a NUL byte; rill reports what U+FFFD would give.

### The drop-in component

```sh
node conformance/dom/dom-diff.mjs --build --config code          # DOM, per document
node conformance/dom/dom-diff.mjs --config code --variant prefix  # props variants
node conformance/dom/dom-diff.mjs --config code --stream every=40 # streamed prefixes
bun conformance/dom/ssr.tsx --config code                         # renderToString
node conformance/dom/interactions.mjs                             # buttons, menus, modal
node conformance/remend.mjs --fuzz 100000 --streams 400           # remend against remend
node conformance/blocks.mjs --fuzz 3000                           # the block splitter
bun conformance/dom/ssr.tsx --definitions --every 13 --corpus chat # reference-link blocks
```

`js/test/doubling.test.ts` and `remend.rs`'s `stream_cost` test stream answers of n, 2n, 4n
and 8n characters and fail if an update's cost grows with the answer.

`dom-diff.mjs` renders each document with Streamdown 2.7.1 and with rill's component in
Chromium, waits until both stop changing (highlighting included), and compares their
`innerHTML` and the lengths of their text nodes (parsed HTML joins adjacent text that
React keeps apart). Configurations are `default`, `code` (`@streamdown/code`), `math`, `cjk` and
`all`; variants cover `prefix`, `controls={false}`, `linkSafety` off, `lineNumbers`
off, a `components` override, `dir="rtl"`, `mode="static"`, `caret`, a custom
`BlockComponent` around `Block`, `translations` for the code controls' labels, and a copy
icon of your own. With `--stream` it streams the chat corpus into one
root with `isAnimating`, comparing after every Nth character.

### Benchmarks

```sh
cargo run --release -p rill --example bench    # rill, native
node conformance/bench.mjs                     # the JavaScript pipeline, same document
```

The browser numbers at the top come from `bench/browser`: a chat page built with each
engine, traced in Chromium at 10× CPU throttle on a phone-sized screen, with interleaved
runs and bootstrap intervals. Its README has a script per scenario and the check that the
final HTML is byte-identical.

### Layout

- `crates/rill-cmark`: the pulldown-cmark 0.13.4 fork, with every change from upstream
  marked `rill:`.
- `crates/rill`: `mdast.rs` (the syntax tree), `hast.rs` (HTML tree, sanitize, harden,
  plugins), `autolink.rs`, and `wasm` (the boundary the JavaScript package reads).
- `crates/rill/src/markup.rs`: Streamdown's DOM as HTML segments for the drop-in;
  `remend.rs`: remend 1.4.0.
- `js/`: the `@2018kguo/rill` npm package (the drop-in component, loader, opcode
  decoder, Streamdown plugins, React hook) and its test against Streamdown's pipeline.
- `conformance/`: the differential harnesses (trees, DOM, SSR, interactions, remend), the
  probe and the benchmark.
- `demo/perf`: the main-thread benchmark (production React, CPU throttling).
- `bench/browser`: the trace-based browser benchmark behind the README's table.
- `demo/`: the demo page.

## License

MIT. `crates/rill-cmark` is derived from pulldown-cmark (MIT, Copyright 2015 Google Inc.).
