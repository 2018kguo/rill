# rill stream: a bottom-up engine

Status: design, Nov 2026. Branch `rewrite/stream`.

## Why

rill today is a chain of exact ports: remend, then marked's block splitter, then remark
(micromark → MDAST), then remark-rehype (HAST), then rehype-raw/sanitize/harden, then
Streamdown's React components. Each layer is fast for what it is, but an update pays for
all of them, plus React reconciling every block. Measured on a 40 KB answer streamed
256 characters at a time, React and the block lexer dominate; the parser itself runs at
~44 MiB/s natively because it builds two trees with positions it never needs for the
drop-in.

The contract does not change: **the DOM Streamdown 2.7.1 renders, byte for byte, at every
streamed step**. What changes is how we get there. The current pipeline becomes the
**oracle** (it is already verified against Streamdown); the runtime is rewritten from the
output backwards.

## Targets (all at once, not traded against each other)

| Axis | Target |
|---|---|
| Streaming update time (browser, per update) | lowest of any streaming Markdown renderer measured |
| Heap after streaming | < 1 MB for a 40 KB answer |
| Raw throughput (Markdown → HTML string) | ≥ 1 GiB/s native, ≥ 600 MiB/s in wasm |
| Base download (gzip, React component + wasm) | < 60 KB |
| Correctness | Streamdown's DOM exactly; CommonMark + GFM spec |

## Shape

```
text ──► Session.update(text)  (one wasm call, or none on a fast path)
           │  stable boundary: completed blocks never re-scanned
           │  repair (remend-exact) of the open tail only
           │  block scan (Streamdown's split) + inline render, straight to HTML bytes
           ▼
        { reset, addition, pending }   HTML of newly completed blocks / the open tail
           │
           ▼
        DomPatcher(root)   React owns only the root <div>; patcher appends `addition`,
                           morphs `pending` in place (text grows by appendData),
                           interactive parts wired by delegation on the root
```

### 1. Session (Rust, `crates/rill-stream`)

- **Stable boundary.** A block is complete once a later line proves it cannot change
  (a blank line with nothing open, as the incremental remend/blocks already decide). Its
  HTML is emitted once as `addition` and never again. Only the source after the boundary
  is scanned on the next update.
- **Fast paths without the parser** (checked in JS before crossing into wasm):
  - plain words appended to an open paragraph: escape and append to the last text node;
  - lines appended inside an open code fence: escape and append to the code text.
  Each fast path has a precondition that is cheap and conservative (no syntax
  characters, no line that could start a block); anything else takes the full path.
- **One pass, no trees.** Block structure and inline content are scanned once and written
  as Streamdown's HTML (classes, `data-streamdown`, React's escaping and attribute order)
  into one output buffer. No MDAST, no HAST, no positions, no per-node allocation.
  Inline delimiter resolution uses a fixed stack over the paragraph's bytes.
- **Byte-level scanning.** Table-driven classes per byte, `memchr`-style skips to the
  next special character, ASCII fast paths, UTF-16 offsets computed only at the
  boundary.

### 2. Fallback

Anything the new engine does not yet reproduce (raw HTML, footnotes and link
definitions, math, an app's own plugins or components) makes the session report
`fallback` and the message renders through the current pipeline, loaded lazily. The
set shrinks over time; the oracle decides when a construct moves in.

### 3. DOM side (JS, small)

- React renders one root `<div>` with Streamdown's root props (class, caret style).
- Server rendering and hydration: the root's initial HTML is the session's full output.
- Interactive parts (code-block copy/download, table menus, link-safety dialog) are
  plain HTML wired by one delegated listener; the dialog/menus mount through a portal
  only when opened.
- Highlighting: worker results patch the code element of a block by index.

### 4. Size

`rill-stream` has no dependency on the current crates; its wasm is the base download.
The current pipeline's wasm and Streamdown's JS load only on fallback.

## Proof

- **Exhaustive streams** (`js/test/exhaustive.test.ts`): the new component joins as a
  third engine; every stream over the small alphabets must equal Streamdown at every
  step, and the fallback must be taken, not guessed.
- **Differential fuzz** against the current pipeline (fast, in Rust/Node): random
  documents and random chunkings; the session's `stable + pending` must equal the
  current markup's HTML for the same text.
- **Big-O**: the doubling tests count bytes scanned per update; scanning must not grow
  with the answer.
- **Benchmarks**: paired, interleaved, same harness for every library.

## Phases

0. Oracle plumbing: an HTML string of the current pipeline's Streamdown DOM in Node, and
   a differential fuzz runner.
1. Session skeleton + paragraphs, headings, emphasis/strong/code spans, links/autolinks,
   hr, fenced and indented code: the common chat answer.
2. Lists, blockquotes, tables, task lists.
3. DOM patcher + React root + fast paths; swap into the drop-in behind eligibility.
4. Remaining constructs move in from fallback; size and throughput passes.
