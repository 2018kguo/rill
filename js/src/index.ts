// rill in JavaScript: the core (`createRill`) plus the prebuilt WebAssembly, which
// renders Streamdown's pipeline with its default options, `plugins.math` and
// `plugins.cjk`.
import {
  createRill,
  type RenderedHast,
  type RenderMarkup,
  type Rill,
} from "./core.ts";

export * from "./core.ts";

type Glue = {
  default(options: { module_or_path: URL }): Promise<unknown>;
  initSync(options: { module: BufferSource | WebAssembly.Module }): unknown;
  renderHast(source: string, math: boolean, cjk: boolean): RenderedHast;
  renderMarkup: RenderMarkup;
  remendStream(appended: string, reset: boolean): number;
  remendMiddle(): string;
  definitions(source: string, math: boolean, cjk: boolean): Uint32Array;
  splitsAt(source: string, math: boolean, cjk: boolean, at: number): boolean;
};

const glue = (): Promise<Glue> => import("../wasm/rill.js");

let loading: Promise<Rill> | undefined;
let loaded: Rill | null = null;

/** rill once `loadRill` or `initRill` has finished, else `null`. */
export function loadedRill(): Rill | null {
  return loaded;
}

function ready(bindings: Glue): Rill {
  if (!loaded) {
    loaded = createRill(
      bindings.renderHast,
      bindings.renderMarkup,
      { ranges: bindings.definitions, splitsAt: bindings.splitsAt },
      { stream: bindings.remendStream, middle: bindings.remendMiddle }
    );
    warmUp(loaded);
  }
  return loaded;
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
 * Renders a sample a few times in idle moments after loading: the engine compiles
 * WebAssembly in tiers and tiers a function up once it has run a while, and memory
 * grows to its working size, so the first real render runs at full speed.
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

/** Fetches the prebuilt WebAssembly (Streamdown's pipeline) once and returns rill. */
export function loadRill(): Promise<Rill> {
  loading ??= (async () => {
    const bindings = await glue();
    await bindings.default({
      module_or_path: new URL("../wasm/rill_bg.wasm", import.meta.url),
    });
    return ready(bindings);
  })();
  // A failed fetch (offline, deploy skew) is retried on the next call.
  loading.catch(() => {
    loading = undefined;
  });
  return loading;
}

/** Instantiates the prebuilt WebAssembly from its bytes (Node, SSR, tests). */
export async function initRill(
  wasm: BufferSource | WebAssembly.Module
): Promise<Rill> {
  const bindings = await glue();
  bindings.initSync({ module: wasm });
  const rill = ready(bindings);
  loading ??= Promise.resolve(rill);
  return rill;
}
