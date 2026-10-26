# How rill makes Streamdown faster

[Streamdown](https://github.com/vercel/streamdown) is the Markdown component many AI chat
apps use to show answers as they arrive. rill keeps Streamdown's component and produces
the same page, with much less work behind it. This page walks through where that work
goes in a chat UI and what rill changes, with the numbers from
[benchmarks.md](benchmarks.md).

## Where the time goes

A chat UI renders Markdown in two situations.

**While an answer streams**, the text grows a few characters at a time, often dozens of
times a second, and the page shows each state. The end of the text is usually
half-written: `**bol` should already look bold, a link whose URL hasn't arrived should
read as text, and a code block whose closing fence hasn't arrived should still look like
code.

**When a conversation opens**, every past answer renders at once. A long chat can hold
dozens of answers with code, tables and lists.

Streamdown already avoids the most obvious waste. It splits an answer into blocks
(paragraphs, lists, tables, code blocks) and remembers the finished ones, so a streamed
update only re-renders the block being written. Everything below is about the work that
is left.

## What rill changes

### Updates don't re-read the whole answer

Before Streamdown renders an update, it does two passes over the entire answer so far:

1. **The repair.** A small library called remend closes whatever is still open at the
   end of the text, so the half-written `**bol` gets its closing `**` and renders as
   bold. To know what is open, it scans the whole text: how many `**` there are, whether
   a code fence is still open, which brackets are unclosed.
2. **Finding blocks.** A Markdown lexer (marked) reads the whole text again to find
   where each block starts and ends.

Both passes cost time in proportion to the answer's length, and they run on every
update. A 20 KB answer arriving 24 characters at a time takes about 850 updates, and an
average update scans about 10 KB twice. Over the stream that is 17 MB of text read to
show 20 KB.

rill does both passes incrementally:

- **The repair** keeps the text it saw last time and a settled prefix: the text up to a
  blank line where nothing is open (no code span, no math, no bracket waiting for its
  partner). Nothing written after that point can change how the prefix repairs, so its
  counts are computed once and kept. Each update repairs only the text after the
  prefix, and returns only what changed in the repaired text.
- **Finding blocks** reuses the blocks the last update found, as long as the new text
  still starts with them and they ended in a blank line, and lexes only from the first
  block that could still change. A few things could still reach back across a blank
  line, like a link definition whose label isn't closed yet; rill checks for those and
  lexes the whole text when one shows up.

In practice the lexer reads about 400 characters per update, from the start of a 56 KB
answer to its end. rill's tests stream answers of 2, 4, 8 and 16 thousand characters and
count the characters each update hands to the parser and the lexer. A test fails if that
count grows with the answer, so a regression fails the build.

### One pass instead of a chain of plugins

Streamdown turns Markdown into a page through a chain of JavaScript plugins: parse the
text into a syntax tree, convert it into an HTML tree, then clean that tree (rebuild any
raw HTML, remove anything unsafe, neutralize dangerous links). Each step walks the tree
again and allocates a new one.

rill does the same steps in one pass, in Rust compiled to WebAssembly. Each step is a
port of the package it replaces, rule for rule. For plain Markdown this part is 20 to 30
times faster per render than the JavaScript chain. Answers with math or raw HTML gain
less, because KaTeX and the HTML5 parser still run in JavaScript.

### Finished text is plain HTML, not React components

React keeps a component, with bookkeeping, for every element it renders: every
paragraph, list item and link. On each update it compares them to decide what changed.
Text that has finished arriving will never change, so for it that bookkeeping is pure
cost.

rill hands the browser finished text as plain HTML and has React build only the parts
that respond to the user: code block buttons, table menus, and links that open
Streamdown's confirmation dialog. A code block's copy button starts as plain HTML too and
becomes Streamdown's real button the first time it's pressed. After opening a long chat,
rill's JavaScript heap is about a third of Streamdown's (13.9 MB against 41.8 MB in the
benchmark).

### Tables stream a row at a time

While a table streams, the table is the block being written, so Streamdown re-renders
all of it, every row, on each update. A table row's Markdown is one line, and once the
line has ended, that row renders the same whatever comes after it. rill keeps the rows
whose lines have ended and renders only the header and the line still arriving. Streaming
the 118-row periodic table takes 14.5 times less JavaScript than in Streamdown.

### Answers with footnotes stream in pieces

Footnotes are numbered across the whole answer (the third footnote cited gets a 3,
wherever its definition is), and their definitions are collected at the bottom. So
Streamdown can't split an answer that has footnotes: it renders it as one block, and
every update re-renders the whole answer. The same goes for reference-style links
(`[1]: https://...`), whose definitions can sit anywhere.

rill renders such an answer as the pieces Streamdown would otherwise have cut it into.
Each piece is numbered after the footnotes cited before it, and the footnote list
renders as a last piece of its own. An update then re-renders the piece being written and,
if the definitions changed, the list. In a CPU profile at 4× throttling, streaming
the 56 KB chapter from the benchmark with one footnote near the top used to cost 16.5
seconds, against 10.3 for the chapter as it is; it now costs 10.6.

### Code highlighting off the main thread

Coloring code takes a tokenizer that runs a grammar's regular expressions over every
line. rill does it with [rill-highlight](https://github.com/2018kguo/rill-highlight), a
Rust port of Shiki (the highlighter Streamdown uses) that produces the same tokens, in a
Web Worker so the page stays responsive. Results that arrive together are applied in
one React update. With `@2018kguo/rill/code`, the page doesn't download Shiki either: on
the benchmark page that is 58 KiB of gzipped JavaScript.

## How we know it's the same

rill is only worth using if switching changes nothing on the page. It is checked against
Streamdown itself:

- The same documents render through both, and the resulting DOMs are compared: every
  element, attribute and class, and how the text is split into text nodes.
- Streamed answers are compared at every step as they arrive, not only when they finish.
- Every string up to a fixed length over Markdown's tricky characters (`*`, `>`, `|`,
  `` ` ``, newlines) goes through both parsers. A mismatch then shows up as the smallest
  input that triggers it.
- Server rendering, hydration, and the interactive parts (buttons, menus, the link
  dialog) are checked in a browser.

When one of these finds a difference, the document that showed it joins the test corpus.
[conformance.md](conformance.md) has the commands.

## What we learned

**Once the JavaScript is gone, the browser's own work is most of what's left.** Opening
the 18-answer chat on the throttled profile, rill's main thread is busy for 1.6 seconds,
and 0.64 of those are the browser computing styles, laying out and painting, which is
the same work for both engines because the DOM is the same. The CSS tip in the README
(`content-visibility: auto` on message rows) attacks that part: it lets the browser skip
laying out messages that are off screen.

**Measure before porting.** It was tempting to move every remaining piece of JavaScript
into Rust. Measuring first changed the plan. The block lexer, for example, reads about
400 characters per update, so a Rust version would save almost nothing, and Streamdown's
own copy of the lexer stays in the bundle either way. The native parser renders a 97 KB
answer in 3.6 ms, so tuning it further wouldn't show up on screen either. The time went
into the changes above instead.

**The worst costs grow with the answer.** A cost that depends on the answer's length
looks fine on a short answer and gets worse the longer the answer runs. The footnote
case is an example: split into quarters, streaming the footnoted chapter cost 2.2, 3.4,
5.0 and 6.0 seconds, each quarter more than the one before. That is why the tests double
the answer length and check the cost per update, and why the benchmark streams a 56 KB
answer instead of a 2 KB one.

**Comparing every streamed step finds bugs that comparing final pages misses.** Two
examples: a footnote cited twice lost its second back-link while a later
definition was still arriving, and a comment cut off mid-stream split one line break
into two text nodes in animated messages. A single render of the finished answer
showed neither; the step-by-step comparison caught both.
