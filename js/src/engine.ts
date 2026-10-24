// An engine: a rill build (its WebAssembly, made by `rill::wasm_exports!`) together with
// the JavaScript remark and rehype plugins it reproduces. `<Streamdown engine>` renders
// with the build where it can and with exactly those plugins where it cannot, so the two
// never come from different places.
import type { PluggableList } from "unified";

import { argsKey, createRill, type Args, type Glue, type Rill, type RillBuild } from "./core.ts";
import { Lru } from "./prefix.ts";

/** The remark and rehype plugins a build reproduces, as `<Streamdown>` takes them. */
export type Pipeline = {
  readonly remarkPlugins: PluggableList;
  readonly rehypePlugins: PluggableList;
};

/** A build's `wasm-bindgen --target web` glue module. */
export type GlueModule<A extends Args = []> = Glue<A> & {
  default(options: { module_or_path: URL | string | Request }): Promise<unknown>;
  initSync(options: { module: BufferSource | WebAssembly.Module }): unknown;
};

export type EngineDefinition<A extends Args = []> = {
  /** The build's glue, imported on demand: `() => import("./pkg/build.js")`. */
  glue: () => Promise<GlueModule<A>>;
  /** Where its WebAssembly is, from your own module so every bundler sees it:
   * `() => new URL("./pkg/build_bg.wasm", import.meta.url)`. */
  wasm: () => URL | string;
  /**
   * The JavaScript remark and rehype plugins the build reproduces for `args`: what
   * Streamdown renders with where rill does not, and what the build is checked
   * against (`checkEngine`). Called once per distinct `args`.
   */
  plugins: (...args: A) => Pipeline;
  /**
   * The entry of `rehypePlugins` that renders the build's math (rehype-katex, or
   * `[rehypeKatex, options]`), for a build whose own options turn math on: rill leaves
   * math elements to it, with the options it has in the list.
   */
  katex?: unknown;
};

export type Engine<A extends Args = []> = {
  /** The JavaScript plugins for `args`: the same objects for the same values. */
  plugins(...args: A): Pipeline;
  /** The rehype-katex entry of `plugins(...args).rehypePlugins`, if the build has one. */
  math(...args: A): unknown;
  /** rill for `args` once the WebAssembly has loaded, else `null`. */
  rill(...args: A): Rill | null;
  /** Fetches and instantiates the WebAssembly once (browsers). A failure is reported
   * once and retried on the next call. */
  load(): Promise<void>;
  /** Instantiates the WebAssembly from its bytes (Node, server rendering, tests). */
  init(wasm: BufferSource | WebAssembly.Module): Promise<void>;
  /** Called once the WebAssembly is ready. */
  subscribe(listener: () => void): () => void;
};

const attacherOf = (plugin: unknown): unknown => (Array.isArray(plugin) ? plugin[0] : plugin);

export function defineEngine<A extends Args = []>(definition: EngineDefinition<A>): Engine<A> {
  let build: RillBuild<A> | null = null;
  let loading: Promise<void> | undefined;
  let warmed = false;
  const listeners = new Set<() => void>();
  const pipelines = new Lru<string, { pipeline: Pipeline; math: unknown }>(256);

  const ready = (glue: GlueModule<A>) => {
    if (build) return;
    build = createRill(glue);
    for (const listener of listeners) listener();
  };

  const pipelineFor = (args: A) => {
    const key = argsKey(args);
    let found = pipelines.get(key);
    if (!found) {
      const pipeline = definition.plugins(...args);
      let math: unknown;
      if (definition.katex !== undefined) {
        math = pipeline.rehypePlugins.find(
          (plugin) => plugin === definition.katex || attacherOf(plugin) === definition.katex
        );
        if (math === undefined) {
          throw new Error("rill: the engine's `katex` is not in its rehypePlugins");
        }
      }
      found = { pipeline, math };
      pipelines.set(key, found);
    }
    return found;
  };

  return {
    plugins: (...args) => pipelineFor(args).pipeline,
    math: (...args) => pipelineFor(args).math,
    rill(...args) {
      if (!build) return null;
      const rill = build.bind(...args);
      if (!warmed) {
        warmed = true;
        warmUp(rill);
      }
      return rill;
    },
    load() {
      if (build) return Promise.resolve();
      loading ??= (async () => {
        const glue = await definition.glue();
        await glue.default({ module_or_path: definition.wasm() });
        ready(glue);
      })();
      // A failed fetch (offline, deploy skew) is retried on the next call.
      loading.catch(() => {
        loading = undefined;
      });
      return loading;
    },
    async init(wasm) {
      if (build) return;
      const glue = await definition.glue();
      glue.initSync({ module: wasm });
      ready(glue);
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

// A little of everything Streamdown renders.
const SAMPLE = `# Heading **bold** *em* ~~del~~ \`code\`

Text with a [link](https://example.com), www.example.com and a footnote[^1].

- item one
- [x] done
  1. nested

> quote

| a | b |
|:--|--:|
| 1 | **2** |

\`\`\`ts
const x = 1;
\`\`\`

[^1]: note
`;

/**
 * Renders a sample a few times in idle moments once the first page uses rill: the
 * engine compiles WebAssembly in tiers and tiers a function up once it has run a while,
 * and memory grows to its working size, so the first real render runs at full speed.
 */
function warmUp(rill: Rill) {
  if (typeof setTimeout !== "function") return;
  let rounds = 16;
  // `spine` renders are not cached, so each round runs.
  const options = { prefix: "", linkSafety: true, ssr: false, overrides: "", spine: true };
  const round = () => {
    try {
      rill.markup?.(SAMPLE.repeat(8), {}, options);
      rill.render(SAMPLE, {});
    } catch {
      return;
    }
    rounds -= 1;
    if (rounds > 0) schedule();
  };
  const idle = (globalThis as { requestIdleCallback?: (run: () => void) => unknown })
    .requestIdleCallback;
  const schedule = () => (idle ? idle(round) : setTimeout(round, 0));
  schedule();
}
