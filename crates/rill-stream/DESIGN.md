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

### Renderer

One full parser, `rill-cmark` (micromark's grammar as Streamdown configures it), feeds a
writer that emits tokens for Streamdown's DOM in the same pass: no MDAST or HAST. The
writer owns everything the exact pipeline does in its tree passes (mdast-util-to-hast's
whitespace text between block children, tight-list paragraphs, Streamdown's
components: classes, link buttons, code block and table shells, images, task lists,
footnotes, URL hardening).

The hand-written block and inline scanners stay as a fast path for the blocks they
render exactly (headings, rules, tight lists, paragraphs with code spans and emphasis),
including the incremental inline resume. Anything else goes to the full writer. Both
write the same tokens, so the diff and the DOM side do not care which ran.

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
- A Rust differential against the exact `rill` pipeline for static content.
- `js/test/stream*.test.ts`: Streamdown itself in happy-dom, over small alphabets and
  after every streamed update.
