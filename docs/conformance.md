# Conformance: how rill stays identical to Streamdown

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
known differences are listed under [Conformance](#the-pipeline-harness).

## The pipeline harness

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

## The drop-in component harness

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
`BlockComponent` around `Block`, `translations` for the code controls' labels, a copy
icon of your own, `animated` (`animated`, `animated-options`, on one clock for both
sides), and the common plugins in the page's own lists (`breaks`, `slug`, `external`,
`remark-math`, `math-single`, `extras`). With `--stream` it streams the chat corpus into
one root with `isAnimating`, comparing after every Nth character. `conformance/diff.mjs`
checks the plugins' trees too (`--config breaks`, `slug`, `external`, `math1`, `extras`).
