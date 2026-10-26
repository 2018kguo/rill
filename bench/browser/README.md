# Browser benchmark

The benchmark behind the README's table: Streamdown 2.7.1 and rill's drop-in, each on the
same chat page (`stage/`), in Chromium with production React.

## Run it

From the repository root, build rill's WebAssembly and install the workspace (rill's
sources, `js/wasm/rill_bg.wasm` and `@2018kguo/rill-highlight` come from there):

```sh
npm install && npm run wasm
cd bench/browser
npm install && npx playwright install chromium
npm run build                  # dist/streamdown and dist/rill
```

One `npm run` per scenario (`npm run bench` runs all four):

| Script | Scenario | Stage query |
|---|---|---|
| `bench:hist-distinct` | Open a chat of 18 different long answers | `scene=history&chat=distinct&overlay=0` |
| `bench:hist-code` | Open a code-heavy chat (the LRU-cache answer, 18 times) | `scene=history&answers=lru-cache&overlay=0` |
| `bench:table118` | Stream the 118-element periodic table, 24 characters a frame | `scene=stream&doc=elements-118&chunk=24&overlay=0` |
| `bench:chapter` | Stream the Rust Book's chapter 4 (56 KB), 24 characters a frame | `scene=stream&doc=ownership-chapter&chunk=24&overlay=0` |

Options go after `--`, for example `npm run bench:hist-distinct -- --runs 3 --warmups 1
--throttle 4,10`. Defaults: 15 runs after 2 warm-ups, 10× CPU throttle, a 412×915 screen
at 2.625× (a budget Android phone), results in `results.json`. `--scenario name=query`
runs any other stage query; `--engine name=<dist>` compares other builds.

An engine of your own (a custom build, other props per answer such as `mode="static"`)
is a module like `stage/engine.rill.ts` exporting `Streamdown`, `engineName` and
`answerProps(settled, text)`: `bun build.ts mine=path/to/engine.ts` builds it into
`dist/mine`, then `--engine mine=dist/mine`.

Each scenario's final HTML is the same in both engines:

```sh
npm run domcheck -- hist-distinct    # each engine's hash; exits 1 at the first difference
```

For `hist-distinct` both print `d243ab18c779`.
It runs unthrottled. At 10× throttle on a loaded machine, Shiki's 500 ms tokenizing limit
per line can cut a long line short in Streamdown, so its DOM node count in a benchmark
report can vary a little between runs.

What each page loads (`rill-code` is rill with `code` from `@2018kguo/rill/code`, which
`bun build.ts rill-code` builds from `stage/engine.rill-code.ts`):

```sh
SOURCEMAP=1 bun build.ts streamdown rill rill-code
node bytes.mjs --by package streamdown rill rill-code   # gzipped, and by source package
```

## What is measured

Every number comes from a Chrome trace (`@paulirish/trace_engine`) or the browser's own
Long Animation Frames entries, never from timers in the page:

- **wall**: from the page's `stage:start` mark to the end of the run.
- **busy**: main-thread `RunTask` time in that window, split into **style/layout**,
  **paint**, **GC** and **script** (the rest). **script + GC** is the engine's own share;
  layout and paint are the same on both sides, since the DOM is. Work in workers (rill
  highlights code in one) is reported separately, not hidden.
- Frames and dropped frames, LoAF blocking time, the JS heap after a full GC, and the DOM
  nodes in the answer.

Runs are interleaved, alternating which engine goes first, each in a fresh browser
context. A report prints medians and a 95% bootstrap interval (seeded) on each ratio of
medians: wall, busy, script, GC, script + GC, LoAF blocking, heap.

**The stage's done rule.** The page is done once the content is all there (history: every
turn rendered; stream: the last character handed to the engine), the DOM has not changed
for 600 ms, the main thread had no frame gap over 100 ms in those 600 ms (a long task
may be highlighting that has not reached the DOM yet), and every code block is
highlighted (no line left as a raw token colored `inherit`). The last DOM change is the
run's `finished` time.

**The end of a run** is where js-framework-benchmark ends it: once that last DOM change
is painted, at the end of the first main-thread paint `Commit` after it.

`overlay=0` turns off the page's fps counter, whose repaints would be the harness's cost,
not the engine's.

## Content

- `stage/ownership.md`, `stage/ownership-chapter.md`, 15 of the 18 answers in
  `stage/chat.json`, and `stage/img/trpl04-*.svg` are from
  [The Rust Programming Language](https://github.com/rust-lang/book) (MIT or Apache-2.0),
  listings inlined.
- `stage/elements-118.md` is derived from
  [Periodic-Table-JSON](https://github.com/Bowserinator/Periodic-Table-JSON) by
  Bowserinator (CC BY-SA 3.0), and is itself under CC BY-SA 3.0.
- The other answers (`lru-cache.md`, `elements.md`, `japan.md`) were written for this
  benchmark.

See `NOTICE`.
