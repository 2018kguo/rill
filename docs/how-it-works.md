# How rill makes Streamdown faster

[Streamdown](https://github.com/vercel/streamdown) is the Markdown component many AI chat
apps use to show answers as they arrive. rill keeps Streamdown's component and produces
exactly the same page, but does far less work to get there. This page explains where the
work goes in a chat UI, what rill does about it, and what we learned along the way.

## Where the time goes

A chat UI renders Markdown in two situations:

- **While an answer streams.** The text grows a few characters at a time, many times a
  second, and the page has to show each new state. Half-written text is tricky: `**bol`
  could become bold, and a half-written table row could change the table, so the
  renderer has to treat the end of the answer as provisional.
- **When a conversation opens.** Every past answer renders at once, often dozens of long
  ones.

Streamdown already avoids the most obvious waste: it splits an answer into blocks
(paragraphs, lists, tables, code) and only re-renders the block being written. What's
left is still a lot of work per update, and on a phone it shows up as stutter while text
arrives and a blank screen while a chat opens.

## What rill does differently

### It remembers its work between updates

To find where blocks start and end, and to repair half-written syntax (closing a
dangling `**` so it renders as bold), Streamdown re-reads the whole answer on every
update. That cost grows with the answer: the thousandth token of a long answer costs far
more than the tenth.

rill keeps what it learned from the last update. The repair runs only on the text that
was just added, and finding blocks re-reads only the part that can still change. So the
cost of an update stays flat however long the answer gets. rill's tests stream answers
of doubling length and fail if an update's cost grows with them.

### One pass instead of a chain of plugins

Streamdown turns Markdown into a page through a chain of JavaScript plugins: parse the
text into a tree, convert that into an HTML tree, then clean it (normalize raw HTML,
remove anything unsafe, block dangerous links). Each step walks the whole tree again.

rill does the same steps in one compiled pass, in Rust running as WebAssembly. Each step
is ported from the package it replaces, rule for rule. On its own, this part is 20 to 30
times faster per render for plain Markdown (less for math or raw HTML, which still need
JavaScript libraries).

### Finished text is plain HTML, not React components

React builds a component for every element it renders: every paragraph, link and
list item gets bookkeeping that React keeps in memory and compares on every update. For
text that has finished arriving, none of that is needed: it will never change.

rill hands the browser finished text as plain HTML and lets React build only the parts
that are interactive: code block buttons, table menus, links that open a confirmation.
Less React bookkeeping means less memory: after a long chat opens, rill's JavaScript
heap is about a third of Streamdown's.

### Tables keep their finished rows

When an answer streams a table, the table is the block being written, so Streamdown
re-renders the whole table on every update. rill keeps the rows that have finished and
renders only the new line. Streaming a long table costs a fraction of what it did.

### Answers with footnotes stream in pieces

Footnotes are numbered across the whole answer, so Streamdown keeps an answer with
footnotes in one block, and every update re-renders all of it. The longer the answer,
the slower each token (the total work grows with the square of the length). The same
happens with reference-style links (`[1]: https://...`).

rill renders such an answer as the pieces it would otherwise have been split into, and
carries the footnote numbering from one piece to the next, so only the last piece
changes. Streaming a 56 KB answer with one footnote near the top went from 16.5 to
10.6 seconds of work on a throttled phone profile, about the same as the answer without
the footnote.

### Code highlighting off the main thread

Coloring code is expensive. rill does it with
[rill-highlight](https://github.com/2018kguo/rill-highlight), a Rust port of Shiki (the
highlighter Streamdown uses) that produces the same colors, in a background thread, and
applies a batch of results in one update. With `@2018kguo/rill/code`, the page doesn't
download Shiki at all, which saves 58 KiB of JavaScript on the benchmark page.

## How we know it's the same

"Same page" is the point of a drop-in: switching renderers usually means subtly
different lists, links or emphasis, and nobody wants to audit that. rill is checked
against Streamdown itself:

- the same documents rendered by both, compared element by element, attribute by
  attribute, down to how text is split into nodes;
- every prefix of streamed answers compared as they arrive, not just the final page;
- every string up to a fixed length over Markdown's tricky characters (`*`, `>`, `|`,
  `` ` ``, newlines), which turns each mismatch into its smallest possible example;
- server rendering, hydration, buttons, menus and the link confirmation dialog.

Every bug found this way becomes a test document. [conformance.md](conformance.md) has
the details.

## What we learned

- **Most of what's left is the browser's own work.** Once the JavaScript is gone, what
  remains when a chat opens is the browser parsing, styling and laying out the page,
  which is the same for both since the page is the same. That's why one line of CSS
  (`content-visibility: auto` on message rows, in the README) helps rill a lot and
  Streamdown little: it lets the browser skip laying out messages that are off screen.
- **Speed came from doing less, not from the language.** The parser renders a 97 KB
  answer in under 4 ms natively. The wins came from not redoing work: remembering
  between updates, skipping React for static text, keeping finished table rows,
  splitting footnoted answers. Rewriting more of the remaining JavaScript in Rust (we
  measured the block splitter) would not have made anything noticeably faster.
- **Measure growth, not just speed.** The worst problems were costs that grow with the
  answer's length (re-reading the whole answer, footnoted answers re-rendering whole).
  They look fine on short answers and only hurt on long ones, so rill's tests double the
  answer length and check that the cost per update doesn't follow.
- **Compare against the original, constantly.** Small differences hide in rare shapes:
  a footnote numbered after a late definition, a comment still being typed at the end
  of an answer. Checking every streamed prefix against Streamdown found them before
  users did.
