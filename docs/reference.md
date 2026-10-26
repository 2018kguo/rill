# Reference

The drop-in's behavior in detail, the API of each entry point, and how to bring plugins
of your own. For what rill is and how to switch, see the [README](../README.md).

## The drop-in component

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
in one pass, as Streamdown's static mode does, through rill too. With `animated`
(Streamdown 2's word-by-word fade while an answer streams, with any of its options),
rill runs the animation plugin's pass in Rust on the same shared timeline, so each word
gets the span, duration and delay Streamdown would give it. (One difference: Streamdown
keeps a block's elements as its words come and go, and React leaves an empty `style`
behind on an element that had one. A block that turns to Streamdown's `Block` mid-answer,
as raw HTML arrives, starts over with new elements, so those leftovers, which show
nothing, are missing there.)

The `remarkPlugins` and `rehypePlugins` pages add most often render on the fast path
too, with no Rust: remark-breaks, remark-math with rehype-katex (`$x$` or only `$$x$$`),
rehype-slug and rehype-external-links, after Streamdown's own plugins, as published or
minified (see [Plugins on the prebuilt build](#plugins-on-the-prebuilt-build)).

Anything rill doesn't reproduce exactly renders with Streamdown's own `Block`, the same
way: raw HTML that needs the HTML5 parser (rill handles comments, phrasing tags, `<img>`
and `<br>` with or without attributes, and tags sanitize removes around Markdown, such as
custom elements or the Rust Book's `<Listing>`), `allowedTags`, other
`remarkPlugins`/`rehypePlugins` (bring them as a [custom build](#custom-builds) instead),
`urlTransform` and the other Markdown filters, `fallbackComponent`, nesting deeper than
256 levels (quotes, lists, emphasis, raw tags), `mode="static"` with `dir`, math while
`animated` streams, and every block until the WebAssembly has loaded (and on the server
without `initRill`). In development rill says so once per reason in the console, and
`setFallbackListener` hears every one (see [Fallbacks](#fallbacks)).

The other tiers stay:

- `@2018kguo/rill/react`: `useRill(plugins?)` returns `remarkPlugins`/`rehypePlugins` for
  Streamdown's own component (`<Streamdown plugins={plugins} {...useRill(plugins)}>`):
  rill parses, Streamdown's React renders.
- `@2018kguo/rill/plugins` (also exported from `/streamdown`): `rillPlugins`, the same
  plugins when you load rill yourself.

## Setup per bundler

The WebAssembly and the highlighting worker load by themselves, from URLs the bundler
emits (`new URL(..., import.meta.url)`): no files to copy, no config.

- **Next.js** (App Router; Turbopack and webpack, `next dev` and `next build`): nothing
  to configure. Import it in client components or render it straight from a server
  component, as you would Streamdown: the server renders it with Streamdown's own
  pipeline and the page hydrates without a warning
  ([examples/nextjs](../examples/nextjs)).
- **Vite 8**, and frameworks on it such as React Router (SSR included): nothing to
  configure ([examples/vite](../examples/vite), [examples/react-router](../examples/react-router)).
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

## JavaScript

`@2018kguo/rill` ships the WebAssembly prebuilt, so you don't need Rust to use it.

- `@2018kguo/rill/streamdown`: `Streamdown` and `Block`, [the drop-in](#the-drop-in-component), plus everything
  `streamdown` exports. `setRillHighlight(false)` keeps Shiki for code;
  `setHighlightWasm(url)` points at `rill_highlight.wasm` (from
  `@2018kguo/rill-highlight/dist/`) where a bundler doesn't place it by itself, as bun
  doesn't (Vite and webpack do), and starts the highlighting worker;
  `preloadHighlighting()` starts it without one.
- `@2018kguo/rill/code`: `code` and `createCodePlugin({ themes })`, `@streamdown/code`'s,
  highlighting with rill-highlight instead of Shiki. The page loads only its language list;
  rill-highlight's own plugin loads if the worker can't start.
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
  them to change. Write a Rust plugin ([Options and plugins](#options-and-plugins)) or work on the HTML tree with `after`.
- **`remarkRehypeOptions`.** rill renders Streamdown's setting (`allowDangerousHtml`).
- **`allowedTags`.** Streamdown widens its sanitize schema only with its own default
  `rehypePlugins`, so with rill's the extra tags are removed.

Without rehype-raw in `rehypePlugins` (Streamdown then shows raw HTML as text), and with
`disableAutolinkProtocols` or any other remark plugin `<Streamdown>` adds, every document
renders in JavaScript.

The WebAssembly loads with `new URL("../wasm/rill_bg.wasm", import.meta.url)` as soon as
the browser evaluates `@2018kguo/rill/streamdown`; see [Setup per bundler](#setup-per-bundler)
for where that resolves.

## Rust

```rust
use rill::{Kind, Options, Syntax};

let rendered = rill::render("# Hello *world*", &Options::default())?;
assert_eq!(rendered.kind, Kind::Complete);

let mdast_json = rill::parse_json("- [x] done", Syntax::default())?;
```

`rill::mdast::parse_arena` returns the syntax tree itself, an arena that borrows the
source text instead of copying it.

## Plugins on the prebuilt build

A GitHub code search in October 2026 (366 repositories that pass `<Streamdown>` lists of
their own, out of 435 sampled; repositories per package, counting per-package searches)
found these the most common: remark-gfm (77), remark-breaks (50), remark-math (42) with
rehype-katex (40), Streamdown's own rehype-raw, rehype-harden and rehype-sanitize listed
again (37, 30, 27), rehype-slug (11), remark-directive (9),
remark-github-blockquote-alert (7) and rehype-external-links (5); 210 of the 366 also pass
a plugin of their own. The drop-in renders these lists on the fast path with the prebuilt
WebAssembly:

| In `remarkPlugins` / `rehypePlugins` | rill |
|---|---|
| Streamdown's `defaultRemarkPlugins` / `defaultRehypePlugins` (spread, picked, or the same `remark-gfm` and `rehype-raw` imported again) | as Streamdown's, remarkCodeMeta optional; rehype-sanitize also with no options (hast-util-sanitize's schema) |
| remark-breaks 4 | every line ending in text is a `<br>` |
| remark-math 6 (`singleDollarTextMath` either way) with rehype-katex 7 (any options) | math parsed in Rust, KaTeX in JavaScript; `plugins.math` with `singleDollarTextMath: true` too |
| rehype-slug 6 (`prefix`) after the chain | github-slugger's ids |
| rehype-external-links 3 (`target`, `rel`, `protocols`) after the chain | `rel` and `target` on links to other sites |

Plugins are found by name or, when a minifier renamed them, by the shape of their code,
and each is then run once on a probe document next to rill's own version: if their
output differs, the list renders with Streamdown and says so. Anything else in the lists
(remark-directive, a plugin of your own, harden or sanitize with options of your own)
renders all of the message's blocks with Streamdown, reported as `remarkPlugins` or
`rehypePlugins` with the plugins' names.

## Fallbacks

Each block rill leaves to Streamdown's own `Block` has a reason. In development
(`process.env.NODE_ENV !== "production"`), rill logs each reason once per page, for
example:

```
[rill] rehypePlugins given (rehypeMermaid): these messages render with Streamdown (slow path). See https://github.com/2018kguo/rill#fallbacks.
```

Production builds drop the messages. For telemetry, `setFallbackListener` (from
`@2018kguo/rill/streamdown`) hears every fallback, in production too, and costs nothing
when unset:

```ts
import { setFallbackListener } from "@2018kguo/rill/streamdown";
setFallbackListener((reason, detail) => track("rill-fallback", { reason, detail }));
```

| Reason | Why |
|---|---|
| `remarkPlugins`, `rehypePlugins` | lists with plugins rill does not know (`detail` names them; see [Plugins on the prebuilt build](#plugins-on-the-prebuilt-build)) |
| `allowedTags`, `literalTagContent`, `disableAutolinkProtocols` | props that change Streamdown's plugin lists |
| `allowElement`, `allowedElements`, `disallowedElements`, `skipHtml`, `unwrapDisallowed`, `urlTransform`, `remarkRehypeOptions` | Markdown filters rill does not run |
| `fallbackComponent` | the prop is given |
| `static-dir` | `mode="static"` with `dir` |
| `engine-lists` | `<Streamdown engine>` given its own `remarkPlugins` or `rehypePlugins` |
| `math-twice` | math in the engine or the lists, and `plugins.math` |
| `animated-block-component`, `animated-options`, `animated-math` | `animated` with a `BlockComponent` of your own, with an `animation` or `easing` only a CSS parser reads (quotes, `;`, `!`), or math while it streams |
| `raw-html` | raw HTML that needs the HTML5 parser |
| `too-deep`, `too-long`, `surrogates` | nesting deeper than 256 levels, a block over 128K characters, lone surrogates or NUL |
| `plugin-declined` | a custom build's Rust plugin (or rehype-slug after KaTeX, on a heading with math) left the block to JavaScript |
| `math-without-katex` | a build's math with no KaTeX plugin to render it |
| `incomplete-fence-components`, `incomplete-diagram` | a code fence still streaming next to components of your own or diagram renderers |
| `unsupported-shape` | an element or property the markup does not reproduce |
| `loading` (not logged), `server`, `load-failed`, `trapped`, `error` | the WebAssembly is not loaded yet, server rendering without `initRill`, the load failed, it crashed (rill stops calling it), or a render threw |

## What runs where

| Stage | Package ported | Runs in |
|---|---|---|
| Parse | remark-parse 11, remark-gfm 4, Streamdown's remarkCodeMeta; `plugins.math`: remark-math 6 (either dollar rule); `plugins.cjk`: remark-cjk-friendly 2.3 (and its strikethrough) and its autolink boundary; remark-breaks 4 | Rust |
| MDAST → HAST | remark-rehype 11 (`allowDangerousHtml`) | Rust |
| Raw HTML | rehype-raw 7 | Rust without raw HTML, else JavaScript |
| Sanitize | rehype-sanitize 6, Streamdown 2.7.1's schema | Rust |
| Harden | rehype-harden 1.1.8 with Streamdown's options | Rust |
| After the chain | rehype-slug 6, rehype-external-links 3 | Rust |
| Animation | `animated`: Streamdown 2.7.1's animate plugin | Rust |
| Math | `plugins.math`: rehype-katex 7 | JavaScript |

`render` reports how far it got:

- `complete`: the whole chain ran, and the tree is final.
- `raw`: the document has raw HTML (or a link inside a link), which needs parse5. Run
  rehype-raw, rehype-sanitize and rehype-harden (and with math, rehype-katex) on the tree.
- `math` (with `math` on): everything is final but the math elements, for rehype-katex,
  which Streamdown runs last.

`conformance/pipeline.mjs` has these tails ready to use.

## Options and plugins

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
rehype plugin also works unchanged on rill's output (`after`, under [JavaScript](#javascript)), so a Rust plugin is
only worth it for work that has to happen earlier, such as on the syntax tree.

## Custom builds

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
