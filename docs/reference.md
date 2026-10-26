# Reference

The details behind the one-import switch: what the drop-in does differently, how each
bundler picks it up, every entry point, which plugins stay fast, and when a message falls
back to Streamdown's own code. For what rill is and how to switch, see the
[README](../README.md); for plugins of your own, [customizing.md](customizing.md).

## The drop-in component

`@2018kguo/rill/streamdown` exports everything `streamdown` 2.7.1 exports, so the rest of
your imports (`Block`, `CodeBlock`, `defaultComponents`, the types) keep working from the
new path. That includes a `BlockComponent` of your own that wraps `Block`: import `Block`
from the new path too and it renders with rill.

The component takes the same props and renders the same DOM, with the same behavior: copy
and download buttons, table menus, the link safety modal, the caret, server rendering and
hydration, `components` overrides and `plugins`.

### What changes behind it

Streamdown is still the component. Its root, its contexts and its memoization are its own,
and it splits an answer into the same blocks, with the same splitter. rill takes over at
the level of one block:

- **Rendering a block.** rill turns the block's Markdown into HTML in Rust. Text that will
  never change becomes plain HTML inside one React element. React builds only the parts
  people interact with (code block frames, tables, images, links that open the safety
  modal), using Streamdown's own exported components. A code block's copy and download
  buttons start as plain HTML and become Streamdown's real buttons on the first press,
  and that press still counts.
- **Code highlighting.** With `@streamdown/code` or `@2018kguo/rill/code` in `plugins`,
  code is colored by [rill-highlight](https://github.com/2018kguo/rill-highlight), a Rust
  port of Shiki's tokenizer that gives the same tokens, in a Web Worker. The worker starts
  with the first code block; `preloadHighlighting()` starts it earlier. Colors that arrive
  together are applied in one React render.
- **Streaming.** Each update costs about what the block being written costs, however long
  the answer is. The repair that closes unfinished Markdown (remend) only looks at the
  text after the last point where nothing was left open, and the splitter only re-reads
  the blocks that can still change. An answer that Streamdown keeps as one block (because
  it has footnotes or `[1]: https://...` link definitions) renders as pieces, and only the
  last piece changes. [how-it-works.md](how-it-works.md) explains each of these.
- **Opening a conversation.** A message that isn't streaming renders all its blocks from
  one component instead of one per block, and what it computes from its text is cached
  by content, so mounting it again (scrolling back, switching chats) is cheap.
- **`mode="static"`** renders the whole text in one pass, as Streamdown's static mode
  does, through rill.
- **`animated`** (Streamdown's word-by-word fade while an answer streams, with any of its
  options) runs in Rust on the same timeline, so each word gets the span, duration and
  delay Streamdown would give it.

One difference you can't see: Streamdown reuses a block's elements as words come and go,
and React leaves an empty `style` attribute on an element that had one. When a block
switches to Streamdown's own `Block` mid-answer (because raw HTML arrived), it starts over
with new elements, so it doesn't have those empty attributes.

### What still renders with Streamdown

A block rill can't reproduce exactly renders with Streamdown's own `Block`, and looks the
same. That covers raw HTML that needs a full HTML5 parser, plugin lists rill doesn't know,
the filtering props (`allowedTags`, `urlTransform` and the rest), and every block until
the WebAssembly has loaded. [Fallbacks](#fallbacks) lists every reason and how to watch
for them.

rill does handle the raw HTML chat answers usually contain: comments, inline tags such as
`<kbd>` or `<sup>`, `<img>` and `<br>` with or without attributes, and tags that
Streamdown's sanitizer removes anyway, such as custom elements or the Rust Book's
`<Listing>`.

## Setup per bundler

The WebAssembly and the highlighting worker load from URLs the bundler emits
(`new URL(..., import.meta.url)`), so there are no files to copy.

- **Next.js** (App Router; Turbopack and webpack, `next dev` and `next build`): nothing to
  configure. Use it in client components, or render it from a server component as you
  would Streamdown. The server renders with Streamdown's own pipeline and the page
  hydrates without a warning ([examples/nextjs](../examples/nextjs)).
- **Vite 8**, and frameworks built on it such as React Router (server rendering
  included): nothing to configure ([examples/vite](../examples/vite),
  [examples/react-router](../examples/react-router)).
- **Vite 7 and older**: add one plugin. Vite 7's dependency pre-bundling loses those URLs,
  and its build needs workers that are ES modules:

  ```ts
  // vite.config.ts
  import { rill } from "@2018kguo/rill/vite";
  export default defineConfig({ plugins: [react(), rill()] });
  ```

- **bun build**: bun doesn't emit the highlighter's WebAssembly. Serve
  `@2018kguo/rill-highlight/dist/rill_highlight.wasm` yourself and pass its URL to
  `setHighlightWasm`.

CI builds each example in each of these modes and opens it in Chromium. It checks that the
page is server-rendered where the app renders on the server, that there's no console error
or hydration warning, that both WebAssembly files are served as `application/wasm`, that
code is highlighted in the worker, and that a streamed answer rendered entirely through
rill (`examples/run.sh`, `examples/smoke.mjs`). Other bundlers that handle
`new URL(..., import.meta.url)` and module workers (webpack 5 on its own, Rollup, Parcel)
should work the same way, but CI doesn't check them.

## Entry points

`@2018kguo/rill` ships the WebAssembly prebuilt, so you don't need Rust to use it.

| Import | What it gives you |
|---|---|
| `@2018kguo/rill/streamdown` | `Streamdown` and `Block`, plus everything `streamdown` exports. Also `setFallbackListener`, `setRillHighlight(false)` to keep Shiki for code, `setHighlightWasm(url)` for bundlers that don't place the highlighter's WebAssembly themselves, and `preloadHighlighting()` |
| `@2018kguo/rill/code` | `code` and `createCodePlugin({ themes })`, like `@streamdown/code`'s, but highlighting with rill-highlight instead of Shiki. The page loads only the list of languages |
| `@2018kguo/rill/engine` | `defineEngine`, for [custom builds](#custom-builds). `rillEngine` is the prebuilt build, defined the same way |
| `@2018kguo/rill/conformance` | `checkEngine`, which compares a custom build with the JavaScript plugins it copies |
| `@2018kguo/rill/vite` | `rill()`, the plugin for Vite 7 and older |
| `@2018kguo/rill/react` | `useRill(plugins?)`, for using rill under Streamdown's own component (below) |
| `@2018kguo/rill/plugins` | `rillPlugins`, the same without the hook (below) |
| `@2018kguo/rill` | `loadRill()` downloads the WebAssembly once; `initRill(bytes)` starts it from bytes (Node, tests); `rill.render(markdown, { math, cjk })` returns the HTML tree directly |

The drop-in starts downloading the WebAssembly as soon as the browser evaluates
`@2018kguo/rill/streamdown`.

### Using rill under Streamdown's own component

Before the drop-in, rill plugged into Streamdown as a remark/rehype plugin pair, and that
still works: rill parses, Streamdown's own React renders. It's slower than the drop-in
(React builds every element), but needs no change to the component:

```tsx
<Streamdown plugins={plugins} {...useRill(plugins)}>{markdown}</Streamdown>
```

Or, when you load rill yourself:

```ts
import { loadRill } from "@2018kguo/rill";
import { rillPlugins } from "@2018kguo/rill/plugins";

const plugins = rillPlugins({ rill: await loadRill(), after: [rehypeSlug] });
// <Streamdown {...plugins}>{markdown}</Streamdown>   (memoize it)
```

`rillPlugins` takes:

- `plugins`: the `plugins` prop you give `<Streamdown>` (its `math` and `cjk`). If the two
  disagree, documents render in JavaScript, with the same output. With math on, rill
  renders each formula with KaTeX once and caches it.
- `after`: rehype plugins of your own, run on the finished HTML tree.
- `maxSourceLength`: longer documents render in JavaScript. The default is 128K UTF-16
  units, because WebAssembly memory grows to fit the largest document and never shrinks.
- `onRender` and `onError` callbacks.

What this path can't do:

- **Mix with your own `remarkPlugins` or `rehypePlugins` props.** Pass rill's lists as
  they are and add rehype plugins through `after`. (If you replace `remarkPlugins`, the
  document still renders safely, all in JavaScript.)
- **Run remark plugins.** rill parses in Rust, so there's no JavaScript syntax tree for
  them to change. Write a [Rust plugin](#options-and-plugins), or work on the HTML tree
  with `after`.
- **Take `remarkRehypeOptions`.** rill always uses Streamdown's setting
  (`allowDangerousHtml`).
- **Take `allowedTags`.** Streamdown only widens its sanitizer for these with its own
  default `rehypePlugins`, so with rill's lists the extra tags are removed.

Without rehype-raw in `rehypePlugins` (Streamdown then shows raw HTML as text), or with
`disableAutolinkProtocols` or any other remark plugin `<Streamdown>` adds, every document
renders in JavaScript.

## Plugins on the prebuilt build

To pick which plugins to support without a custom build, we searched GitHub in October
2026 for repositories that pass `<Streamdown>` plugin lists of their own (366 of 435
sampled). The most common: remark-gfm (77 repositories), remark-breaks (50), remark-math
(42) with rehype-katex (40), Streamdown's own rehype-raw, rehype-harden and
rehype-sanitize listed again (37, 30, 27), rehype-slug (11), remark-directive (9),
remark-github-blockquote-alert (7) and rehype-external-links (5). 210 of the 366 also pass
a plugin of their own.

The drop-in renders these lists through rill with the prebuilt WebAssembly:

| In `remarkPlugins` / `rehypePlugins` | rill does |
|---|---|
| Streamdown's `defaultRemarkPlugins` / `defaultRehypePlugins` (spread, picked, or the same `remark-gfm` and `rehype-raw` imported again) | what Streamdown does; remarkCodeMeta can be left out; rehype-sanitize also with no options (hast-util-sanitize's schema) |
| remark-breaks 4 | every line ending in text becomes a `<br>` |
| remark-math 6 (`singleDollarTextMath` either way) with rehype-katex 7 (any options) | parses the math in Rust and leaves KaTeX to JavaScript; `plugins.math` with `singleDollarTextMath: true` too |
| rehype-slug 6 (`prefix`) after Streamdown's plugins | the same ids as github-slugger |
| rehype-external-links 3 (`target`, `rel`, `protocols`) after Streamdown's plugins | `rel` and `target` on links to other sites |

rill finds these by name, or by the shape of their code when a minifier renamed them.
Then it runs each one once on a test document next to its own version, and if the outputs
differ, that list renders with Streamdown and says so. Anything else in the lists
(remark-directive, a plugin of your own, harden or sanitize with options of your own)
sends the message to Streamdown, reported as `remarkPlugins` or `rehypePlugins` with the
plugins' names.

## Fallbacks

Each block rill leaves to Streamdown's own `Block` has a reason. In development
(`process.env.NODE_ENV !== "production"`), rill logs each reason once per page:

```
[rill] rehypePlugins given (rehypeMermaid): these messages render with Streamdown (slow path). See https://github.com/2018kguo/rill#fallbacks.
```

Production builds drop these messages. To count fallbacks in production, register a
listener. It hears every one and costs nothing when unset:

```ts
import { setFallbackListener } from "@2018kguo/rill/streamdown";
setFallbackListener((reason, detail) => track("rill-fallback", { reason, detail }));
```

| Reason | When |
|---|---|
| `remarkPlugins`, `rehypePlugins` | the lists hold plugins rill doesn't know (`detail` names them; see [the table above](#plugins-on-the-prebuilt-build)) |
| `allowedTags`, `literalTagContent`, `disableAutolinkProtocols` | props that change Streamdown's plugin lists |
| `allowElement`, `allowedElements`, `disallowedElements`, `skipHtml`, `unwrapDisallowed`, `urlTransform`, `remarkRehypeOptions` | Markdown filters rill doesn't run |
| `fallbackComponent` | the prop is given |
| `static-dir` | `mode="static"` together with `dir` |
| `engine-lists` | `<Streamdown engine>` was also given its own `remarkPlugins` or `rehypePlugins` |
| `math-twice` | math is on in the engine or the lists, and in `plugins.math` too |
| `animated-block-component`, `animated-options`, `animated-math` | `animated` with a `BlockComponent` of your own; with an `animation` or `easing` that only a CSS parser could read (quotes, `;`, `!`); or math while the answer streams |
| `raw-html` | raw HTML that needs the HTML5 parser |
| `too-deep`, `too-long`, `surrogates` | nesting deeper than 256 levels (quotes, lists, emphasis, raw tags), a block over 128K characters, lone surrogates or NUL |
| `plugin-declined` | a custom build's Rust plugin declined the block (also rehype-slug after KaTeX, on a heading with math) |
| `math-without-katex` | a build renders math but has no KaTeX plugin |
| `incomplete-fence-components`, `incomplete-diagram` | a code fence still streaming, with components of your own or diagram renderers |
| `unsupported-shape` | an element or attribute rill's markup can't reproduce |
| `loading` (not logged), `server`, `load-failed`, `trapped`, `error` | the WebAssembly hasn't loaded yet; server rendering without `initRill`; the download failed; it crashed (rill stops calling it); or a render threw |

## Custom builds

Plugins of your own can run in Rust too, compiled with rill into your own WebAssembly.
[customizing.md](customizing.md) walks through writing one, pairing it with your
JavaScript plugins, testing the two against each other and shipping it.
`examples/custom-build` is a complete one (math, three plugins, two arguments), checked by
`js/test/engine.test.ts` and the DOM harness (`--variant engine`).

The lower-level path takes a build too: `myEngine.rill(...args)` is the `Rill` object for
`rillPlugins({ rill })` and `rill.render(source)`.

## Rust

```rust
use rill::{Kind, Options, Syntax};

let rendered = rill::render("# Hello *world*", &Options::default())?;
assert_eq!(rendered.kind, Kind::Complete);

let mdast_json = rill::parse_json("- [x] done", Syntax::default())?;
```

`rill::mdast::parse_arena` returns the syntax tree itself, in an arena that borrows the
source text instead of copying it.

### What runs where

| Step | Package it ports | Runs in |
|---|---|---|
| Parse | remark-parse 11, remark-gfm 4, Streamdown's remarkCodeMeta; with `plugins.math`, remark-math 6 (either dollar rule); with `plugins.cjk`, remark-cjk-friendly 2.3 (and its strikethrough) and its autolink boundary; remark-breaks 4 | Rust |
| Syntax tree to HTML tree | remark-rehype 11 (`allowDangerousHtml`) | Rust |
| Raw HTML | rehype-raw 7 | Rust when there's no raw HTML, else JavaScript |
| Sanitize | rehype-sanitize 6, with Streamdown 2.7.1's schema | Rust |
| Harden links and images | rehype-harden 1.1.8, with Streamdown's options | Rust |
| After Streamdown's plugins | rehype-slug 6, rehype-external-links 3 | Rust |
| Animation | with `animated`, Streamdown 2.7.1's animate plugin | Rust |
| Math | with `plugins.math`, rehype-katex 7 | JavaScript |

`render` reports how far it got:

- `complete`: every step ran, and the tree is final.
- `raw`: the document has raw HTML (or a link inside a link), which needs parse5. Run
  rehype-raw, rehype-sanitize and rehype-harden on the tree (and rehype-katex, with math).
- `math` (with `math` on): everything is final except the math elements, which are left
  for rehype-katex, as Streamdown runs it last.

`conformance/pipeline.mjs` has those remaining steps ready to use.

### Options and plugins

`Options` defaults to Streamdown's setup with no `plugins` prop:

- `math`, `cjk`: Streamdown's `plugins.math` (`@streamdown/math`) and `plugins.cjk`
  (`@streamdown/cjk`).
- `legacy_cjk`, `code_meta: false`, `sanitize.schema: Schema::Default`: Streamdown 1.x's
  default plugins, which pages that pass their own lists often still run
  (remark-cjk-friendly 1.2.3 after remark-gfm, no remarkCodeMeta, hast-util-sanitize's
  `defaultSchema`; `diff.mjs --config classic` checks them).
- `harden`: what happens to blocked links and images: `Indicator` (the default),
  `TextOnly` or `Remove`.
- `sanitize.code_classes`: extra `className`s that `<code>` may keep.
- `plugins`: your own plugins.

A plugin implements up to three methods, at the points where a remark or rehype plugin
would run:

```rust
impl rill::Plugin for HeadingIds {
    // After parsing, on the syntax tree (where remark plugins run). Returning
    // Err(Unsupported) leaves the document to the JavaScript plugins.
    fn mdast(&self, tree: &mut rill::mdast::Tree<'_>) -> Result<(), rill::Error> { Ok(()) }
    // On the HTML tree, before sanitizing.
    fn hast_unsanitized(&self, tree: &mut rill::hast::Tree<'_>, root: u32, cx: &rill::Context<'_>) {}
    // On the final HTML tree (where rehype plugins after Streamdown's run), before KaTeX.
    fn hast(&self, tree: &mut rill::hast::Tree<'_>, root: u32, cx: &rill::Context<'_>) {}
}
```

`crates/rill/tests/api.rs` has a complete one that adds heading ids. A JavaScript rehype
plugin also works unchanged on rill's output (`after`, above), so a Rust plugin is only
worth writing for work that has to happen earlier, such as on the syntax tree.
