# Plugins and customizations

Most apps switch to rill by changing one import (see the [README](../README.md)). This
page is for apps that customize Streamdown: they pass their own `remarkPlugins` or
`rehypePlugins`, add Markdown syntax of their own (`@mentions`, citations, links into
the app), or change those settings per message. It explains what happens to those
plugins under rill, how to keep such messages fast, and what to set up before you ship.

## Why plugins need thought

Streamdown runs your plugins on syntax trees it builds in JavaScript. rill builds those
trees in Rust, compiled to WebAssembly, and never hands them to JavaScript, because that
copy is a large part of the time it saves. So a JavaScript remark plugin has nothing to
run on.

rill handles this in three ways, depending on the plugin:

- **It already knows the plugin.** The plugins apps add most often have Rust versions
  inside rill, and it uses them when it recognizes the plugin in your lists.
- **It doesn't know the plugin.** Messages rendered with that list go through
  Streamdown's own code, plugins and all. The page looks exactly as it did; those
  messages just don't get faster.
- **You write the plugin in Rust.** Then everything stays fast. This is a *custom
  build*, described below.

## Which case you're in

| Your `<Streamdown>` | What to do | What renders it |
|---|---|---|
| No `remarkPlugins` or `rehypePlugins`; `plugins` for code, math, CJK or Mermaid at most | Change the import | rill |
| Adds remark-breaks, remark-math with rehype-katex, rehype-slug or rehype-external-links | Change the import | rill, with its Rust versions of these |
| Adds other plugins | Change the import, then decide whether a custom build is worth it | Streamdown until you write one, then rill |
| Uses `allowedTags`, `urlTransform`, `allowElement`, `skipHtml` or the other filters | Change the import | Streamdown, for messages with those props |

None of these paths changes the page. To see how often your app takes the slow path, and
why, register a listener and send it to your telemetry:

```ts
import { setFallbackListener } from "@2018kguo/rill/streamdown";
setFallbackListener((reason, detail) => track("rill-fallback", { reason, detail }));
```

If most fallbacks name one plugin, that plugin is the one to port. The
[reference](reference.md#fallbacks) lists every reason.

## How a custom build works

A custom build is rill compiled together with Rust versions of your plugins. Your
JavaScript plugins don't go away. rill still needs them in three places:

- before the WebAssembly has downloaded, so the first messages don't wait for it;
- on the server, unless you load the build there too;
- for any message your Rust plugin declines (more on that below).

So you give rill both halves together, as an *engine*: the compiled build, plus the
JavaScript plugin lists it reproduces. rill renders each message with the build when it
can and with the lists when it can't.

```text
                    ┌── the build (Rust, WebAssembly): most messages, once loaded
<Streamdown engine> ┤
                    └── your JavaScript plugin lists: before the build loads, on the
                        server, and any message the build declines
```

Because one message can be rendered either way, the two halves have to produce the same
HTML. The JavaScript lists stay the definition of what your Markdown means, and the Rust
plugins copy them. `checkEngine` compares the two on documents you choose (see
[Shipping it](#checking-the-build-against-your-lists)).

[`examples/custom-build`](../examples/custom-build) is a complete one, used through the
rest of this page. It adds `$$` math, `:emoji:` shortcodes, `@name` mentions that link to
a base URL, and `#hashtags`. The base URL and whether hashtags are on are chosen per
message.

### 1. Write the plugins in Rust

A Rust plugin implements up to three methods, one for each point where a JavaScript
plugin could run:

| Method | Runs on | Like a JavaScript |
|---|---|---|
| `mdast` | the syntax tree, right after parsing | remark plugin |
| `hast_unsanitized` | the HTML tree, before unsafe markup is removed | rehype plugin before sanitize |
| `hast` | the final HTML tree | rehype plugin after Streamdown's own |

The emoji plugin replaces `:smile:` in text, so it works on the syntax tree:

```rust
use rill::{Error, Plugin};

pub struct Emoji;

impl Plugin for Emoji {
    // See "Streaming" below.
    fn piecewise(&self) -> bool {
        true
    }

    fn mdast(&self, tree: &mut rill::mdast::Tree<'_>) -> Result<(), Error> {
        // ... replace :smile: in text nodes ...
        Ok(())
    }
}
```

A plugin can decline a message by returning `Err(Error::Unsupported)` from `mdast`. That
message then renders with your JavaScript lists, so the Rust version only needs to
handle the cases it gets right. In the example, the JavaScript hashtag plugin also runs
inside rendered math, which the Rust one doesn't attempt, so the Rust one declines any
message whose math contains a `#`.

### 2. Put everything that varies in one function

Anything that changes between messages (a base URL, a user's settings, whether hashtags
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

`wasm_exports!` generates the functions the npm package calls into, each taking these
arguments. Put this in a crate of type `cdylib` that depends on `rill` with its `wasm`
feature, build it for `wasm32-unknown-unknown`, and run `wasm-bindgen --target web` on the
output. The example's `build.sh` does both.

### 3. Pair the build with your JavaScript lists

```ts
import { defineEngine } from "@2018kguo/rill/engine";

export const myEngine = defineEngine({
  // The JavaScript and WebAssembly that wasm-bindgen wrote.
  glue: () => import("./pkg/my_build.js"),
  wasm: () => new URL("./pkg/my_build_bg.wasm", import.meta.url),
  // The full lists for each set of arguments, Streamdown's defaults included:
  // exactly what you passed to <Streamdown> before.
  plugins: (base: string | null, hashtags: boolean) => ({ remarkPlugins, rehypePlugins }),
  // Only if the build turns math on itself: the rehype-katex entry from those lists.
  katex: rehypeKatex,
});
```

[`examples/custom-build/engine.ts`](../examples/custom-build/engine.ts) shows full lists.

### 4. Render with it

```tsx
<Streamdown engine={myEngine} engineArgs={[baseUrl, true]}>{markdown}</Streamdown>
```

`engineArgs` are the arguments from step 2. They're compared by value, so a new array on
every render is fine. With an engine, `remarkPlugins` and `rehypePlugins` are a type
error, since the engine's lists are the ones in use.

## Shipping it

### Loading the build

The WebAssembly downloads in the background, and messages render with your JavaScript
lists until it arrives, so nothing waits for it. Start the download early, for example
when the chat screen mounts, and report a failure once:

```ts
myEngine.load().catch((error) => reportOnce(error));
```

A failed download is retried on the next call. If it keeps failing, the page keeps
working on the JavaScript lists.

### Server rendering

On the server, messages render with the JavaScript lists unless you load the build there
too (`await myEngine.init(bytes)`). The browser hydrates without a warning either way,
because both halves produce the same HTML.

### Checking the build against your lists

The two halves drift when someone changes a JavaScript plugin and forgets the Rust one.
Catch that in CI with documents shaped like your app's messages:

```ts
import { checkEngine } from "@2018kguo/rill/conformance";

await myEngine.init(readFileSync("pkg/my_build_bg.wasm"));
const report = await checkEngine({ engine: myEngine, args: [baseUrl, true], documents, stream: 20 });
assert.equal(report.identical, report.renders);
```

`checkEngine` renders each document twice, once with Streamdown and your lists and once
with your build, and compares the resulting DOMs. With `stream`, it also compares every
20th prefix of each document, the way an answer looks while it streams. Run it for each
combination of arguments your app uses, and add a document whenever a difference shows
up in production.

### Streaming

Some answers are one block to Streamdown however long they get: an answer with footnotes,
or with reference-style links (`[1]: https://...`), because a definition at the bottom
can change how text near the top renders. Streamdown re-renders the whole answer on
every update. rill renders such an answer in pieces and re-renders only the piece being
written, but only if every plugin in the build says it is `piecewise`.

A plugin is piecewise when its output for a block depends only on that block. The emoji
plugin is: `:smile:` becomes the same emoji wherever it is. A plugin that numbers
citations across the answer is not, because the number of a citation depends on the ones
before it. Leave `piecewise` false for plugins like that. Those answers then render the
way Streamdown renders them, whole on every update.

### Code highlighting

rill highlights code in a Web Worker. It finds the code plugin in your `plugins` prop by
its `name: "shiki"` and `type: "code-highlighter"`. If your app wraps `@streamdown/code`
in a plugin of its own (to load Shiki lazily, say), keep those two fields so rill still
recognizes it. Otherwise use `@2018kguo/rill/code`, which highlights the same way without
shipping Shiki.

### Upgrading rill

A custom build is compiled against one version of the `rill` crate. The npm package
checks, when it loads a build, that the two versions can talk to each other. When you
upgrade `@2018kguo/rill`, rebuild the WebAssembly with the matching crate version in the
same change. A mismatched build refuses to load and the page falls back to your
JavaScript lists, so nothing breaks, but messages are slow until you rebuild.

If CI checks that the committed WebAssembly matches its sources, build it in a pinned
container. Rust puts a hash of the host platform into symbol names, so the same source
built on two different machines gives two different files.
