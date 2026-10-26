# Benchmarks

Every number here was measured on rill 0.4.0 against Streamdown 2.7.1, with
byte-identical final HTML on both sides, unless a section says otherwise. The raw results,
with the machine and versions they came from, are in
[`bench/results/0.4.0/`](../bench/results/0.4.0). Each section ends with the command that
reproduces it.

## In the browser

A chat page (production React, Tailwind, Streamdown's `code` plugin) built once with
Streamdown and once with rill's drop-in, loaded in Chromium with a phone-sized screen
(412×915 at 2.625×) and the CPU throttled 10× (about a budget Android phone). Medians of
5 runs per engine after a warm-up, interleaved; the brackets are 95% bootstrap intervals
on the ratio. Where a cell has two values, the first is Streamdown's.

| Scenario | Finished sooner | Less main-thread JS + GC | Long-frame blocking | JS heap after |
|---|---:|---:|---:|---:|
| Open a chat of 18 long answers (231 KB: code, tables, lists) | 4.4× [3.7–5.4] | 6.6× [5.3–8.0] | 6.1 s to 1.4 s | 41.8 to 13.9 MB |
| Open a code-heavy chat (18 answers, four languages of code) | 8.2× [5.5–9.3] | 11.9× [8.4–13.5] | 5.1 s to 0.5 s | 41.8 to 9.1 MB |
| Stream the 118-element periodic table as one table | 4.2× [4.0–4.4] | 14.5× [13.6–16.5] | 15.4 s to 44 ms | 12.4 to 8.5 MB |
| Stream the Rust Book's ownership chapter (56 KB) | 5.2× [4.8–7.3] | 13.7× [9.5–19.6] | 123 s to 99 ms | 20.3 to 11.6 MB |
| The same chapter below the 18 answers | 4.2× [3.7–5.4] | 9.0× [7.9–12.5] | 141 s to 0.5 s | 51.1 to 19.0 MB |
| The same, at a realistic 2,400 characters a second | stream-bound (1.0×) | 1.8× [1.7–1.8] | 8.6 s to 0.2 s | 48.6 to 18.4 MB |

How to read it:

- **Finished sooner** is wall time from the start of the scenario to the last change on
  screen. Streaming scenarios feed the text as fast as each engine can take it, 24
  characters a frame, except the last row, which feeds it at a fixed pace: there both
  finish when the text does, and what differs is how hard the main thread works and how
  often it blocks.
- **Less main-thread JS + GC** is the engine's own work. Layout and paint are the same
  on both sides (the DOM is the same), so this is the part rill changes.
- **Long-frame blocking** is the time the main thread was busy past 50 ms in a frame,
  from the browser's Long Animation Frames entries: how much a user would feel as
  stutter or delayed input.
- **JS heap after** is the main thread's heap after a full garbage collection, once the
  scenario has settled.

Streaming the chapter, Streamdown dropped 81% of frames and rill 3%.

```sh
npm run bench                         # all six, as above (add -- --runs 15 for tighter intervals)
cd bench/browser && npm run domcheck -- hist-distinct   # the final HTML, byte for byte
```

## What a page loads

The same chat page's JavaScript fetched on load, gzipped:

| Page | JavaScript |
|---|---:|
| Streamdown with `@streamdown/code` | 429.5 KiB |
| rill with `@streamdown/code` | 490.4 KiB |
| rill with `@2018kguo/rill/code` | 431.9 KiB |

rill's WebAssembly (342.9 KiB gzipped, rill and its highlighter) loads alongside without
blocking: until it arrives, Streamdown renders the page as before.

```sh
npm run bench:bytes                   # totals, and where each page's JavaScript comes from
```

## The Markdown pipeline alone

What a page pays per render, without React or the DOM: Streamdown's JavaScript pipeline
(remark, rehype, sanitize, harden, Streamdown's plugins) against rill's package
(WebAssembly, decoding, and whatever JavaScript a document still needs), in Node 24 on
the same machine.

| Document | Streamdown | rill | Faster |
|---|---:|---:|---:|
| A 2 KB answer, Markdown only | 4.39 ms | 0.16 ms | 27× |
| A 96 KB answer, Markdown only | 199 ms | 6.75 ms | 29× |
| A 97 KB answer with math (KaTeX still runs in JavaScript) | 229 ms | 11.1 ms | 21× |
| A 97 KB answer with raw HTML (the HTML5 parser still runs in JavaScript) | 155 ms | 49.7 ms | 3.1× |
| A 7 KB answer streamed, all 293 renders | 1,324 ms | 60.3 ms | 22× |

Natively, rill turns the 97 KB benchmark document into its HTML tree in 2.5 ms, and into
Streamdown's DOM markup in 3.6 ms.

```sh
node conformance/bench-js.mjs         # the table (npm run wasm first)
npm run bench:native                  # rill natively
```

## In a real app: Vercel's ai-chatbot

[vercel/ai-chatbot](https://github.com/vercel/ai-chatbot) (Next.js 16, React 19,
Streamdown with its `cjk`, `code`, `math` and `mermaid` plugins), built for production as
it is and with rill's import changed in the two files that render Markdown, using the
template's mock model. Measured on October 24 2026 with a rill 0.3 development build;
medians of 5 interleaved runs with 95% bootstrap intervals; Streamdown 2.7.1's value first:

| Scenario | CPU | Settled | Main-thread JS + GC | Long-frame blocking |
|---|---:|---:|---:|---:|
| Open a chat of 18 long answers (239 KB) | 4× | 2,991 to 1,942 ms (1.54×) | 2,735 to 1,535 ms (1.78×) | 2,150 to 1,221 ms |
| | 10× | 7,964 to 2,863 ms (2.78× [2.71–2.84]) | 7,338 to 2,214 ms (3.31× [3.23–3.39]) | 6,820 to 2,135 ms |
| Stream an 8 KB answer at 2,400 characters a second | 4× | stream-bound | 3,404 to 2,239 ms (1.52×) | 497 to 4 ms |
| | 10× | 10,056 to 6,427 ms (1.56×) | 9,360 to 5,638 ms (1.66×) | 8,401 to 4,070 ms |

In a real app React also re-renders the app around each answer (the message list, the
chat state), which is most of what remains at 10× and why the gains are smaller than on
the Markdown-only page. That run's memory numbers predate a fix in 0.3 for pages that
stream without `isAnimating` and are left out here.

```sh
# see bench/ai-chatbot/README.md: two production builds, a local Postgres, then
node bench/ai-chatbot/bench.mjs --app streamdown=http://localhost:4801 --app rill=http://localhost:4802
```

## How it's measured

- **From the browser, not timers in the page.** Durations come from Chrome traces
  (`@paulirish/trace_engine`) and the browser's Long Animation Frames entries, after
  [js-framework-benchmark](https://github.com/krausest/js-framework-benchmark).
- **Interleaved runs.** Engines alternate run by run in one browser, so a slow moment on
  the machine hits both, after [Tachometer](https://github.com/google/tachometer); each
  ratio comes with a 95% bootstrap interval.
- **The same page on both sides.** Each scenario's final HTML is compared byte for byte
  (`domcheck`), and the conformance suites check every streamed step (see
  [conformance.md](conformance.md)), so the engines are timed doing the same thing.
- **Recorded with their environment.** `bench.mjs` writes the commit, versions, browser,
  OS and CPU next to the numbers.

Limits worth knowing:

- CPU throttling on a desktop (an Apple M5 Pro here) approximates a slow phone; it isn't
  one. The ratios matter more than the absolute times.
- Chromium only, and one machine.
- The scenarios are chat-shaped by design: long answers with code, tables and lists.
  Answers that are mostly raw HTML gain the least (the HTML5 parser still runs in
  JavaScript), as the pipeline table shows.
