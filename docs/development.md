# Working on rill

## Try it

```sh
npm install
npm run demo          # http://localhost:4173: edit Markdown, compare both, replay a stream
```

The demo builds the WebAssembly, so it needs Rust with the `wasm32-unknown-unknown`
target and `cargo install wasm-bindgen-cli --version 0.2.129`.

## Benchmarks

```sh
npm run bench          # the browser benchmark behind benchmarks.md
npm run bench:bytes    # what each page downloads
npm run bench:native   # rill natively, no browser
node conformance/bench-js.mjs   # the Markdown pipeline alone, in Node
```

[benchmarks.md](benchmarks.md) explains what each one measures. The browser numbers come
from `bench/browser`: a chat page built once with each engine, traced in Chromium on a
phone-sized screen with the CPU throttled 10×. Its README has a script per scenario and
the check that both engines' final HTML is byte-identical.

## Repository layout

- `crates/rill-cmark`: the parser, a fork of pulldown-cmark 0.13.4. Every change from
  upstream is marked with a `rill:` comment.
- `crates/rill`: everything after parsing. `mdast.rs` is the syntax tree; `hast.rs` the
  HTML tree with sanitizing, link hardening and plugins; `autolink.rs` finds bare URLs;
  `markup.rs` writes Streamdown's DOM as HTML for the drop-in; `remend.rs` is the
  streaming repair (a port of remend 1.4.0); `wasm` is what the JavaScript package calls.
- `js/`: the `@2018kguo/rill` npm package: the drop-in component, the WebAssembly loader,
  the decoder for what the WebAssembly returns, the Streamdown plugins and the React hook,
  with tests against Streamdown's own pipeline.
- `conformance/`: the checks against Streamdown (trees, DOM, server rendering, buttons and
  menus, remend) and the tools for looking at one document.
- `bench/browser`: the trace-based browser benchmark behind [benchmarks.md](benchmarks.md).
- `bench/ai-chatbot`: Vercel's ai-chatbot template, built with each engine and traced.
- `demo/`: the demo page; `demo/perf` is an older main-thread benchmark.
- `examples/`: one app per bundler, checked in CI, and `custom-build`, a complete
  [custom build](customizing.md).
