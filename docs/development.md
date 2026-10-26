# Working on rill

## Try it

```sh
npm install
npm run demo          # http://localhost:4173: edit Markdown, compare both, replay a stream
```

The demo needs Rust with the `wasm32-unknown-unknown` target and
`cargo install wasm-bindgen-cli --version 0.2.129`.

---

## Benchmarks

```sh
cargo run --release -p rill --example bench    # rill, native
node conformance/bench.mjs                     # the JavaScript pipeline, same document
```

The browser numbers in [benchmarks.md](benchmarks.md) come from `bench/browser`: a chat page built with each
engine, traced in Chromium at 10× CPU throttle on a phone-sized screen, with interleaved
runs and bootstrap intervals. Its README has a script per scenario and the check that the
final HTML is byte-identical; `bench/browser/bytes.mjs` reports what each engine's page
loads, and with `--by package` where its JavaScript comes from.

## Layout

- `crates/rill-cmark`: the pulldown-cmark 0.13.4 fork, with every change from upstream
  marked `rill:`.
- `crates/rill`: `mdast.rs` (the syntax tree), `hast.rs` (HTML tree, sanitize, harden,
  plugins), `autolink.rs`, and `wasm` (the boundary the JavaScript package reads).
- `crates/rill/src/markup.rs`: Streamdown's DOM as HTML segments for the drop-in;
  `remend.rs`: remend 1.4.0.
- `js/`: the `@2018kguo/rill` npm package (the drop-in component, loader, opcode
  decoder, Streamdown plugins, React hook) and its test against Streamdown's pipeline.
- `conformance/`: the differential harnesses (trees, DOM, SSR, interactions, remend), the
  probe and the benchmark.
- `demo/perf`: the main-thread benchmark (production React, CPU throttling).
- `bench/browser`: the trace-based browser benchmark behind [benchmarks.md](benchmarks.md).
- `bench/ai-chatbot`: Vercel's ai-chatbot template, built with each engine and traced.
- `demo/`: the demo page.
