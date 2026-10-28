<h1 align="center">rill</h1>
<p align="center">Faster streaming Markdown for React, with Streamdown's component API and DOM.</p>
<p align="center">
  <a href="#use-it">Get started</a> ·
  <a href="docs/reference.md">Reference</a> ·
  <a href="docs/benchmarks.md">Benchmarks</a> ·
  <a href="docs/customizing.md">Custom plugins</a>
</p>

Long AI answers should stream smoothly, and opening a long chat should not
freeze the page. rill replaces the work behind
[Streamdown](https://github.com/vercel/streamdown) while keeping its component,
styles and interactive controls. Change the imports below to try it.

In the recorded browser workloads, long chats opened **4–8× sooner** and
used less JavaScript heap. The comparison checks matching final HTML;
[compatibility and fallbacks](#fallbacks) define where the fast path applies.

![Streamdown on the left and rill on the right, recorded at 10× CPU throttle: streaming a table, streaming a long answer, typing while a long chat opens, then the benchmark results](docs/rill-film.gif)

*Real recordings, Streamdown on the left and rill on the right, with Chromium CPU throttling set to 10×. The counters at the top of each pane are measured in the page.*

## Use it

```sh
npm install @2018kguo/rill
```

Change one import:

```diff
- import { Streamdown } from "streamdown";
+ import { Streamdown } from "@2018kguo/rill/streamdown";
```

If you highlight code with `@streamdown/code`, change that import too, and the page
stops shipping Shiki (rill's highlighter gives the same colors):

```diff
- import { code } from "@streamdown/code";
+ import { code } from "@2018kguo/rill/code";
```

`@2018kguo/rill/streamdown` exports everything `streamdown` exports, with the same props
and behavior: copy and download buttons, table menus, the link safety modal, the caret,
server rendering and hydration, `components` overrides and `plugins`.

Bundler setup varies: some configurations need a Vite plugin or an explicit
WebAssembly URL. [Setup per bundler](docs/reference.md#setup-per-bundler)
lists tested configurations; the [reference](docs/reference.md#the-drop-in-component)
records the supported Streamdown API.

If your `<Streamdown>` has `remarkPlugins` or `rehypePlugins` of its own, or Markdown
syntax of its own (mentions, citations, custom links), see
[customizing.md](docs/customizing.md): which plugins rill reproduces as they are, and
how to ship your own as a custom build.

## How much faster

Measured in Chromium on a phone-sized screen, with Chromium CPU throttling set to 10×. This approximates a slower CPU; it is
not a measurement on an Android device. Both engines produce the same final HTML. Each number is the median
of runs that alternate between the two engines.

| Scenario | Done sooner | Less JS execution time | Stutter, Streamdown / rill | Memory, Streamdown / rill |
|---|---:|---:|---:|---:|
| Open a chat of 18 long answers | 4.4× | 6.6× | 6.1 s / 1.4 s | 41.8 / 13.9 MB |
| Open a code-heavy chat | 8.2× | 11.9× | 5.1 s / 0.5 s | 41.8 / 9.1 MB |
| Stream a 118-row table | 4.2× | 14.5× | 15.4 s / 44 ms | 12.4 / 8.5 MB |
| Stream a 56 KB answer | 5.2× | 13.7× | 123 s / 99 ms | 20.3 / 11.6 MB |

*JS execution time* is time the main thread spent running JavaScript, garbage collection
included. *Stutter* is how long the main thread stayed busy past 50 ms in a frame,
which is what a user feels as lag. *Memory* is the JavaScript heap once the page settles.

The table measures a page that does nothing but render Markdown. A real app spends time
on other work too, and that time is the same with either engine: React rendering the
message list, the sidebar and the chat state. So the time ratios come out smaller, while
the memory saved carries over. In Vercel's [ai-chatbot](https://github.com/vercel/ai-chatbot)
template, changing the one import made an 18-answer chat open 3.1× faster (4.4× on the
Markdown-only page), with 3.7× less JS execution time (6.6× there) and a JavaScript heap of
22 MB instead of 49.

[benchmarks.md](docs/benchmarks.md) has every scenario with confidence intervals, page
weight and the raw results. `npm run bench` reruns them.

## What's different

rill keeps Streamdown's component and changes how the work gets done:

- **Reuse completed blocks.** Streamdown re-reads the whole answer on every update, so
  long answers get slower as they grow. rill remembers its work between updates, so
  most updates avoid reparsing the completed blocks.
- **Tables stream a row at a time.** A table being written re-renders only its new row,
  not the whole table.
- **Finished text is plain HTML.** React only builds the interactive parts (buttons,
  menus, links), which keeps memory down.
- **Footnotes don't slow streaming.** Streamdown re-renders a whole answer with footnotes
  on every update; rill renders it in pieces.
- **One compiled pass.** Parsing and cleaning the Markdown runs in Rust (as WebAssembly)
  in one pass instead of a chain of JavaScript plugins, 20 to 30 times faster on its own.
- **Code highlighting off the main thread,** with the same colors as Shiki.

[how-it-works.md](docs/how-it-works.md) explains the parsing, rendering and worker design.

## Compatibility

Switching renderers usually means subtly different lists, links or emphasis. rill is
checked against Streamdown itself: the same documents rendered by both and compared
element by element, every step of streamed answers, every short string over Markdown's
tricky characters, server rendering and the interactive parts. Every bug found that way
becomes a test. See [conformance.md](docs/conformance.md).

## Fallbacks

A few things render with Streamdown's own code instead of rill's, with the same output:
raw HTML that needs a full HTML5 parser, `remarkPlugins` or `rehypePlugins` rill doesn't
reproduce, some filtering props, and everything until the WebAssembly has loaded. In
development rill says so once per reason in the console; `setFallbackListener` hears every
one, in production too:

```ts
import { setFallbackListener } from "@2018kguo/rill/streamdown";
setFallbackListener((reason, detail) => track("rill-fallback", { reason, detail }));
```

The common plugins (remark-breaks, remark-math with rehype-katex, rehype-slug,
rehype-external-links) stay on the fast path, and plugins of your own can run there as a
[custom build](docs/customizing.md#how-a-custom-build-works). Every reason is listed in
[the reference](docs/reference.md#fallbacks).

## One line of CSS for long chats

Once rill renders a chat, most of what's left when one opens is the browser laying out
answers nobody can see yet. Let it skip them: give each settled message's row

```css
.message-row { content-visibility: auto; contain-intrinsic-size: auto 0px auto 240px; }
```

The DOM stays the same (find-in-page and selection work); rows off screen are laid out
when they scroll near.

## Documentation

- [how-it-works.md](docs/how-it-works.md): where the time goes and what rill does about it
- [benchmarks.md](docs/benchmarks.md): every number, how it's measured, how to rerun it
- [customizing.md](docs/customizing.md): your own plugins, custom builds, and shipping them
- [reference.md](docs/reference.md): every entry point, setup per bundler, which plugins run fast, fallbacks
- [conformance.md](docs/conformance.md): how rill is checked against Streamdown
- [development.md](docs/development.md): the demo, the benchmarks and the repository layout

## License

MIT. `crates/rill-cmark` is derived from pulldown-cmark (MIT, Copyright 2015 Google Inc.).
