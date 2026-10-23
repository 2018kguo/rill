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
};

const glue = (): Promise<Glue> => import("../wasm/rill.js");

let loading: Promise<Rill> | undefined;
let loaded: Rill | null = null;

/** rill once `loadRill` or `initRill` has finished, else `null`. */
export function loadedRill(): Rill | null {
  return loaded;
}

function ready(bindings: Glue): Rill {
  loaded ??= createRill(bindings.renderHast, bindings.renderMarkup);
  return loaded;
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
