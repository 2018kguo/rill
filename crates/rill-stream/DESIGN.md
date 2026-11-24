# rill-stream design

rill-stream renders streaming Markdown as Streamdown 2.7.1's exact DOM, patching the
DOM in place on every update. The goal is full coverage at the speed of the plain
paragraph path: no construct takes a slower renderer.

## Pipeline

```text
appended text
  -> rill-remend (incremental)   the repaired text, as Streamdown repairs it
  -> session                     final blocks stay; only the open tail is rendered
  -> renderer                    tail -> tokens (Streamdown's DOM, written directly)
  -> diff                        old tail tokens vs new -> edit program
  -> stream-dom.ts               runs the program against the DOM
```

### Blocks

Streamdown cuts the text with `marked` (v18, GFM) and parses each piece on its own, so
the cuts decide what continues into what (a paragraph ends before a list item with
content, or before a line a table delimiter row follows; a list keeps an indented heading).
`marked.rs` computes the extents of `marked`'s top-level tokens (its block rules and
list and blockquote tokenizers, ported) and the renderer parses each piece alone, root
children joined by a line ending as remark's root joins them. A line that may still
change (unfinished, repaired) can move the cut on the line before it, so the session
keeps the blocks from there open.

### Renderer

Per block, one single-pass parser and writer, no tree: lines go through the open containers
(quotes, list items), start blocks, and add content to the deepest open block; each
block writes Streamdown's DOM as tokens as it closes (mdast-util-to-hast's whitespace
text between block children, tight items' inline paragraphs, Streamdown's components:
classes, link buttons, code block and table shells, images, task lists). Inline content
is scanned once per render, resuming where no later text can change it.

The exact pipeline (`rill`: parser, MDAST, HAST) is the reference for every quirk and
the path for whatever this does not render yet (the whole text, as before). It is not
used per block: building trees costs 3-4x the parse, and the open block is rendered on
every update.

Coverage, in order: links and autolinks; fenced code (shell once, body by lines);
containers (quotes, nested and loose lists); tables (row by row); images, task lists,
strikethrough, raw HTML, indented code, setext headings; footnotes and definitions
(which reset the session).

### Tokens

`kind | id << 2`: OPEN, CLOSE, VOID, TEXT (`id` = UTF-16 length).

- `id < 16`: a fixed tag from `TAGS`.
- `id >= 16`: an interned string, `strings[id - 16]`, kept for the whole session so a
  token means the same thing in every update (the diff compares words):
  - `OPEN`: an element with that open tag (attributes as Streamdown writes them),
    cloned shallow.
  - `VOID`: a whole static subtree (a shell's buttons and their SVG icons), parsed once
    in context and cloned deep.

JavaScript receives the strings interned since the last update.

### Interactive parts

Copy and download buttons, table controls and link buttons have no React behind them:
one delegated listener on the root finds the block from the clicked element and reads
what it needs (code text, table cells, a link's URL from a side table) from the session.

### Code blocks

An open fence renders its shell once and its body line by line, so an update to an
open fence appends or replaces only its last line. Highlighting runs off the main thread
and patches finished lines.

### Session

Final blocks are never rendered again. A link or footnote definition can change earlier
output, so a text with one renders in full on every update (`Reset`).

## Oracles

- `session::tests`: after every chunk, the session's DOM equals the whole repaired text
  rendered from scratch (1M random streams per seed).
- `tests/structure.rs`: random documents' tags and text against the exact `rill`
  pipeline's HAST, block by block (1M per seed).
- `js/test/stream*.test.ts`: Streamdown itself in happy-dom, over small alphabets and
  after every streamed update.

## Pages' own setups

A chat app rarely uses Streamdown's defaults: it overrides components (links, tables,
images, spans), highlights code, and adds its own plugins. Each must stream too, or the
message takes the slow path.

### Components: islands

An overridden tag's element is an island. The session writes its subtree in its raw
HAST form (`<a href>`, `<table>`, `<strong>`: what Streamdown's components receive)
behind an interned open tag marked as an island. The DOM side holds the island's place
with an empty comment (one position, like a text run) and renders the subtree as the
full drop-in renders HAST: `toJsxRuntime` with Streamdown's components and the page's.
React renders it through a portal whose container is a detached holder whose
`appendChild`, `insertBefore` and `removeChild` act after the comment, so the component's
nodes sit where the element belongs, inside the React root (events, context). An island
whose tokens are unchanged keeps its element.

### Code highlighting

`plugins.code` highlights on the stream path through the same worker as the drop-in;
finished lines are patched in place.

### Plugins

The session takes plugins the way `rill` does: a page's own build (its WebAssembly)
links rill-stream with its plugins in Rust, and `engine` hands the drop-in that build.
