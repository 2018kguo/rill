# Conformance: how rill stays identical to Streamdown

A drop-in is only useful if switching changes nothing on the page. This page explains how
rill is checked against Streamdown, what is known to differ, and the commands that run
each check.

## How the parser was built

rill's parser starts from [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark),
a fast Rust Markdown parser. Markdown leaves a lot of room for interpretation, and
pulldown-cmark and micromark (the parser inside Streamdown) disagree on many small
things. So its rules were changed, one at a time, until the two agree on every input we
could generate. The method:

- **Compare against the real thing.** A harness runs Streamdown's own JavaScript pipeline
  and rill on the same input and compares the trees they produce, including where in the
  text each node starts and ends.
- **Try every small input.** Random documents find bugs, but the failing documents are
  long and hard to read. Instead, the harness generates *every* string up to a fixed
  length over a few characters that matter to Markdown, like `*`, `>`, `|` and `` ` ``
  plus newlines. That's millions of documents in seconds, and each mismatch shows up as
  the shortest input that triggers it.
- **Fix the rule, not the case.** Each short mismatch was traced back to micromark's
  source to find the rule behind it. Two examples: emphasis wins over strikethrough when
  it opens first, and a list that ends inside a blockquote ends after the `>`, not before
  it. The fix is that rule, written down in rill, and the next sweep confirms it holds
  for every input, not only the one that failed.

Today every exhaustive sweep matches exactly, across emphasis, lists and quotes, tables,
links, math and URLs, and random documents over a wider set of characters match 99.9% of
the time. The [known differences](#known-differences) are below.

## Checking the page, not only the parser

Matching trees isn't enough. The component also has to build the same DOM, while
streaming as well as at the end, on the server as well as in the browser. So the
component is checked separately:

- The same documents render with Streamdown and with rill in Chromium, and the two DOMs
  are compared: every element, attribute and class, and how the text is split into text
  nodes.
- Streamed answers are compared at every step as they arrive, not only once finished.
  Bugs hide in half-written Markdown: an unclosed code fence, a link whose URL hasn't
  arrived, a footnote cited before its definition.
- Every short document is checked, not only a sample: every stream of up to four or five
  pieces over small alphabets of Markdown's tricky characters (emphasis and links, block
  starts and fences, table rows, link definitions and footnotes, and a long paragraph
  before a fence) streams through both components in one DOM each, a piece at a time
  and cut back between documents, compared after every update
  (`js/test/exhaustive.test.ts`), and the same alphabets render on the server.
- Server rendering and hydration are compared, and the interactive parts (copy buttons,
  table menus, the link modal) are clicked in a browser.
- Tests stream answers of n, 2n, 4n and 8n characters and fail if the cost of one update
  grows with the answer (`js/test/doubling.test.ts`, and the `stream_cost` test in
  `remend.rs`).

Every difference one of these finds becomes a document in the test corpus.

## Known differences

All of them are in rare shapes, and most only change the recorded position of a node, not
the page:

- A tab in a list item's indentation counts as one column, where micromark expands it.
- A block that starts on a continuation line after an empty container, such as indented
  code after a bare `>`, can end one line off.
- micromark miscounts the text before a NUL byte; rill reports what the replacement
  character (U+FFFD) would give.

## Running the checks

### The parser

```sh
cargo build --release --example stdio
node conformance/diff.mjs                                     # the examples
node conformance/diff.mjs --config math+cjk                   # with Streamdown's plugins
node conformance/diff.mjs --alphabet '~*_[]a `' --length 5    # every string over these characters
node conformance/diff.mjs --pieces '["- ","> ","```","\n"]' --length 6
node conformance/diff.mjs --fuzz 3000 --seed 7 --minimize     # random, shrunk to the shortest failure
node conformance/probe.mjs '"> - a\n>"'                       # both trees side by side
```

`diff.mjs` compares the syntax tree (MDAST, without its `data` fields) and the final HTML
tree (HAST), positions included, with Streamdown 2.7.1's pipeline built from the published
`streamdown`, `@streamdown/math` and `@streamdown/cjk` packages. It runs once per `plugins`
setting: `default`, `math`, `cjk` and `math+cjk`. It also checks the common plugins rill
reproduces (`--config breaks`, `slug`, `external`, `math1`, `extras`).

### The component

```sh
node conformance/dom/dom-diff.mjs --build --config code          # DOM, per document
node conformance/dom/dom-diff.mjs --config code --variant prefix  # props variants
node conformance/dom/dom-diff.mjs --config code --stream every=40 # streamed prefixes
bun conformance/dom/ssr.tsx --config code                         # server rendering
node conformance/dom/interactions.mjs                             # buttons, menus, modal
node conformance/remend.mjs --fuzz 100000 --streams 400           # the streaming repair
node conformance/blocks.mjs --fuzz 3000                           # the block splitter
bun conformance/dom/ssr.tsx --definitions --every 13 --corpus chat # answers with link definitions
bun conformance/dom/ssr.tsx --config code --pieces '["*","[","a","\n"]' --length 5 # every short document
DEEPER=1 node --test js/test/exhaustive.test.ts                    # every short stream, one piece longer
```

`dom-diff.mjs` renders each document with Streamdown 2.7.1 and with rill in Chromium,
waits until both stop changing (highlighting included), and compares their `innerHTML` and
the lengths of their text nodes. (Parsed HTML joins neighboring text nodes that React keeps
apart, so `innerHTML` alone would miss that difference.)

- `--config` picks the `plugins`: `default`, `code` (`@streamdown/code`), `math`, `cjk`
  or `all`.
- `--variant` picks props to vary: `prefix`, `controls={false}`, link safety off,
  `lineNumbers` off, a `components` override, `dir="rtl"`, `mode="static"`, `caret`, a
  custom `BlockComponent` around `Block`, `translations` for the code buttons' labels, a
  copy icon of your own, `animated` (`animated`, `animated-options`, on one clock for
  both sides), the common plugins in the page's own lists (`breaks`, `slug`, `external`,
  `remark-math`, `math-single`, `extras`) and a custom build (`engine`).
- `--stream` streams the chat corpus into one root with `isAnimating`, comparing after
  every Nth character.
