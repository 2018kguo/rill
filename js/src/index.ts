// rill in JavaScript: the core (`createRill`) plus the prebuilt WebAssembly, which
// renders Streamdown's pipeline with its default options.
import { createRill, type RenderedHast, type Rill } from "./core.ts";

export * from "./core.ts";

type Glue = {
  default(options: { module_or_path: URL }): Promise<unknown>;
  initSync(options: { module: BufferSource | WebAssembly.Module }): unknown;
  renderHast(source: string): RenderedHast;
};

const glue = (): Promise<Glue> => import("../wasm/rill.js");

let loading: Promise<Rill> | undefined;

/** Fetches the prebuilt WebAssembly (Streamdown's pipeline) once and returns rill. */
export function loadRill(): Promise<Rill> {
  loading ??= (async () => {
    const bindings = await glue();
    await bindings.default({
      module_or_path: new URL("../wasm/rill_bg.wasm", import.meta.url),
    });
    return createRill(bindings.renderHast);
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
  return createRill(bindings.renderHast);
}
