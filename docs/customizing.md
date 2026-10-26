# Plugins and customizations

The README's one-import switch covers apps that use Streamdown's own plugins. This page
is for apps that customize it: their own `remarkPlugins` or `rehypePlugins`, Markdown
syntax of their own (mentions, citations, custom links), or per-message settings. It
covers which path fits, how a custom build works, and what to set up to ship one.

## Which path fits

| Your `<Streamdown>` | What to do | Speed |
|---|---|---|
| No `remarkPlugins`/`rehypePlugins`, `plugins` for code, math, CJK or Mermaid | Change the import | rill |
| Adds remark-breaks, remark-math with rehype-katex, rehype-slug or rehype-external-links | Change the import; rill recognizes these and reproduces them | rill |
| Adds other plugins | Write a custom build (below), or accept that those messages render with Streamdown | rill with a build; Streamdown without |
| Uses `allowedTags`, `urlTransform`, `allowElement`, `skipHtml` or the other filters | These render with Streamdown | Streamdown |

Nothing breaks on the slower paths: a message rill can't reproduce renders with
Streamdown's own code, with the same output. `setFallbackListener` (see
[Fallbacks](reference.md#fallbacks)) tells you how often that happens in production, and
why, so you can decide whether a custom build is worth it.

## How a custom build works

A custom build is rill compiled together with your plugins, written in Rust. You keep
your JavaScript plugins too: the two are paired in an *engine*, and rill uses whichever
can render a given message.

```text
              ┌─ the build (Rust, WebAssembly) ── most messages, once it has loaded
<Streamdown engine>
              └─ your JavaScript plugin lists ── before it loads, on the server,
                                                  and anything the build declines
```

Both sides must produce the same page, so the JavaScript lists stay the definition of
what your Markdown means, and the Rust plugins reproduce them. `checkEngine` compares the
two on documents you give it (below).

The repository has a complete example in
[`examples/custom-build`](../examples/custom-build): `$$` math, an emoji plugin, `@name`
mentions that link to a base URL, and `#hashtags`, with the base URL and a hashtag switch
passed per message.

### 1. Write the plugins in Rust

A plugin implements up to three hooks, at the points where remark and rehype plugins run:

```rust
use rill::{Context, Error, Plugin};

pub struct Emoji;

impl Plugin for Emoji {
    // Its output for a block depends on that block alone (see "Streaming" below).
    fn piecewise(&self) -> bool {
        true
    }

    // After parsing, on the syntax tree (where remark plugins run).
    fn mdast(&self, tree: &mut rill::mdast::Tree<'_>) -> Result<(), Error> {
        // ... edit text nodes ...
        Ok(())
    }

    // On the HTML tree before sanitizing, and on the final HTML tree, also available:
    // fn hast_unsanitized(&self, tree: &mut rill::hast::Tree<'_>, root: u32, cx: &Context<'_>) {}
    // fn hast(&self, tree: &mut rill::hast::Tree<'_>, root: u32, cx: &Context<'_>) {}
}
```

A hook that meets something it can't handle returns `Err(Error::Unsupported)`, and that
message renders with the JavaScript lists instead.

### 2. Define the options once, with their arguments

Everything that varies per message (a base URL, a user's settings, whether citations
show) becomes an argument of one function that returns the build's options:

```rust
pub fn options(base: Option<String>, hashtags: bool) -> rill::Options {
    let mut plugins: Vec<Box<dyn rill::Plugin>> = vec![Box::new(Emoji)];
    if let Some(base) = base {
        plugins.push(Box::new(Mentions { base }));
    }
    if hashtags {
        plugins.push(Box::new(Hashtags));
    }
    rill::Options { math: true, plugins, ..rill::Options::default() }
}

#[cfg(target_arch = "wasm32")]
rill::wasm_exports!(|base: Option<String>, hashtags: bool| options(base, hashtags));
```

`wasm_exports!` generates every entry point the JavaScript package calls, all with these
arguments. The crate is a `cdylib` that depends on `rill` with its `wasm` feature; build
it for `wasm32-unknown-unknown` and run `wasm-bindgen --target web` on the result
(`examples/custom-build/build.sh` does both).

### 3. Pair it with your JavaScript lists

```ts
import { defineEngine } from "@2018kguo/rill/engine";

export const myEngine = defineEngine({
  glue: () => import("./pkg/my_build.js"),
  wasm: () => new URL("./pkg/my_build_bg.wasm", import.meta.url),
  // The lists the build reproduces, for each set of arguments.
  plugins: (base: string | null, hashtags: boolean) => ({ remarkPlugins, rehypePlugins }),
  katex: rehypeKatex, // only if the build turns math on itself
});
```

### 4. Render with it

```tsx
<Streamdown engine={myEngine} engineArgs={[baseUrl, true]}>{markdown}</Streamdown>
```

`engineArgs` compare by value, so a new array on every render is fine. With an engine,
passing `remarkPlugins` or `rehypePlugins` is a type error: the engine's lists are the
ones.

## Shipping it

### Loading

The build's WebAssembly loads in the background; until it arrives, messages render with
your JavaScript lists, so nothing waits on it. Start it early, for example when the chat
screen mounts, and report a lasting failure once:

```ts
myEngine.load().catch((error) => reportOnce(error));
```

A failed load is retried on the next call. If it keeps failing, the page keeps working
on the JavaScript lists.

### Server rendering

On the server, messages render with the JavaScript lists unless you initialize the build
there (`await myEngine.init(bytes)`). Either way the client hydrates without a warning,
because both sides produce the same HTML.

### Checking the build against your lists

Put this in CI with documents in your app's own shapes:

```ts
import { checkEngine } from "@2018kguo/rill/conformance";

await myEngine.init(readFileSync("pkg/my_build_bg.wasm"));
const report = await checkEngine({ engine: myEngine, args: [baseUrl, true], documents, stream: 20 });
assert.equal(report.identical, report.renders);
```

It renders each document with Streamdown and your lists and with your build, settled and
streamed a prefix at a time, and compares the DOMs. Run it for each combination of
arguments your app uses, and add a document every time a difference shows up in
production.

### Streaming

A text with reference-style links (`[1]: https://...`) or footnotes stays one block in
Streamdown. rill streams such a text in pieces only when every plugin in the build says
it is `piecewise`, meaning its output for a block depends on that block alone. A plugin
that counts across the whole text (numbering citations, tracking positions) should leave
`piecewise` false; those texts then render whole on each update, as Streamdown renders
them.

### Code highlighting

If your app wraps `@streamdown/code` in a plugin of its own (to load Shiki lazily, say),
keep the plugin's `name: "shiki"` and `type: "code-highlighter"`: rill recognizes it by
those and highlights its blocks in its worker. Otherwise use `@2018kguo/rill/code`, which
does the same without shipping Shiki.

### Upgrading rill

A custom build is compiled against one version of the `rill` crate, and the npm package
checks that the build speaks its version of the boundary (`rillAbi`). When you upgrade
`@2018kguo/rill`, rebuild your WebAssembly with the matching crate version in the same
change. A mismatched build fails to load and the page falls back to your JavaScript lists,
so it fails safe, but it runs slower until you rebuild.

If your build must be byte-for-byte reproducible (so CI can check the committed
WebAssembly against its sources), build it in a pinned container: Cargo hashes the host
platform into symbol names, so builds on different machines differ.
