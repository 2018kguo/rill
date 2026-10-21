# rill

**Streamdown's Markdown pipeline in Rust.** rill parses Markdown into the exact MDAST
remark builds and renders the exact HAST Streamdown's rehype chain produces, positions
included, at roughly 10× the speed in the browser and 90× natively.

![rill and Streamdown rendering the same answer as it streams in](docs/demo.gif)

[Streamdown](https://github.com/vercel/streamdown) renders Markdown that arrives a few
tokens at a time, so it reparses the whole answer on every chunk. Its pipeline is
unified with remark and rehype plugins; rill reimplements that pipeline as configured
(Streamdown 1.6.11 defaults) and checks itself against it node for node.

## Numbers

97 KB of Markdown (the bundled examples without math or raw HTML, repeated), median of
many runs on an Apple M-series laptop:

| | Streamdown pipeline (Node) | rill (native) |
|---|---:|---:|
| MDAST | 17.7 ms | 0.17 ms (arena), 0.28 ms (JSON) |
| HAST | 29.2 ms | 0.31 ms (JSON) |

In the browser (`npm run demo`), rill compiled to WASM, including `JSON.parse` of its
output, renders each example 8–11× faster than the JavaScript pipeline. Documents with
math or raw HTML gain less, because KaTeX and parse5 still run in JavaScript (see
[What runs where](#what-runs-where)).

Reproduce with `cargo run --release -p rill --example bench` and
`node conformance/bench.mjs`.

## Same output, checked

`conformance/diff.mjs` runs remark and rehype themselves next to rill and compares the
MDAST (minus `data`) and the final HAST, positions included:

```sh
npm install
cargo build --release --example stdio
node conformance/diff.mjs                                     # the examples
node conformance/diff.mjs --alphabet '~*_[]a `' --length 5    # every string over these
node conformance/diff.mjs --pieces '["- ","> ","```","\n"]' --length 6
node conformance/diff.mjs --fuzz 3000 --seed 7 --minimize     # random, with minimal witnesses
```

Exhaustive enumeration was the main tool: every string up to a length over a small
alphabet of delimiters and block starts, so each difference shows up as its smallest
witness and points at one rule of micromark's grammar rather than one input. Over
alphabets for emphasis and strikethrough, containers, tables, links and escapes, lists,
math and URLs, several million generated documents match exactly. Seeded random fuzz
over a wider vocabulary matches 99.9%.

Known differences, all position-only or in rare shapes:

- Tabs in list-item indentation are counted as one column where micromark expands them.
- A block that starts on a lazy line after an empty container, such as indented code
  after a bare `>`, can end one line off.
- micromark miscounts the text before a NUL byte; rill reports what U+FFFD would give.

## Use it

### Rust

```rust
use rill::{Harden, Kind};

let (kind, hast_json) = rill::render("# Hello *world*", &Harden::default())?;
assert_eq!(kind, Kind::Complete);

let mdast_json = rill::parse_json("- [x] done")?;
```

`rill::mdast::parse_arena` gives the MDAST arena itself, which `rill::hast` walks
without copying the source.

### JavaScript (WASM)

```sh
npm run wasm   # needs wasm32-unknown-unknown and wasm-bindgen-cli 0.2.129
```

```js
import init, { renderJson } from "./demo/pkg/rill.js";

await init();
const { kind, tree } = JSON.parse(renderJson(markdown));
// kind "complete": `tree` is Streamdown's final HAST. Render it with
// hast-util-to-jsx-runtime (as react-markdown does) or hast-util-to-html.
```

## What runs where

| Stage | Package ported | |
|---|---|---|
| Parse | remark-parse 11, remark-gfm 4, remark-math 6 (`singleDollarTextMath: false`), remark-cjk-friendly 1.2 (and its strikethrough) | Rust |
| MDAST → HAST | remark-rehype 11 (`allowDangerousHtml`) | Rust |
| Raw HTML | rehype-raw 7 | Rust when there is no raw HTML, else JavaScript |
| Math | rehype-katex 7 | JavaScript |
| Sanitize | rehype-sanitize 6, default schema | Rust |
| Harden | rehype-harden 1.1.8 with Streamdown's options | Rust |

`render` reports how far it got:

- `complete`: the whole chain ran; the tree is final.
- `raw`: the document has raw HTML (or a link inside a link), which needs parse5. Run
  rehype-raw, rehype-katex, rehype-sanitize and rehype-harden on the tree.
- `math`: Streamdown runs KaTeX before sanitizing, so rehype-katex, rehype-sanitize and
  rehype-harden run on the tree.

`conformance/pipeline.mjs` has these tails ready to use.

rehype-harden's block policies are options (`Harden { link_policy, image_policy, .. }`):
`Indicator` (the default, like Streamdown), `TextOnly` or `Remove`.

### Plugins

Any rehype plugin can run on rill's HAST, just as it would run after Streamdown's chain,
so transforms like heading ids, link rewriting or syntax highlighting work unchanged.
remark (MDAST) plugins and syntax extensions don't apply, because the MDAST never leaves
Rust. A native hook for MDAST and HAST transforms is next.

## Inside

- `crates/rill-cmark`: a fork of [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark)
  0.13.4 whose grammar follows micromark. That means CJK-friendly flanking, attention and
  strikethrough resolved in micromark's order, GFM literal autolinks, `$$` math,
  definitions as events, and micromark's interrupt and table rules. Every change is
  marked `rill:`.
- `crates/rill/src/mdast.rs`: events to an arena MDAST. Container ends follow micromark's
  document tokenizer and subtokenize, from a snapshot of the open containers' prefixes.
  Positions stay byte offsets until they're written as UTF-16 units.
- `crates/rill/src/hast.rs`: mdast-util-to-hast, then one fused pass for rehype-raw's
  tree normalization, hast-util-sanitize and rehype-harden.
- `conformance/`: the differential harness, a probe that prints both trees side by side
  (`node conformance/probe.mjs '"*a**"'`), and the benchmark.
- `demo/`: the page in the GIF. Run `npm run demo` and open http://localhost:4173.

## License

MIT. `crates/rill-cmark` is derived from pulldown-cmark (MIT, Copyright 2015 Google Inc.).
