// rill in JavaScript: the core (`createRill`, `defineEngine`) plus the prebuilt build,
// `rillEngine`: Streamdown's pipeline with its default options, `plugins.math` and
// `plugins.cjk`, defined the way any custom build is.
import { defaultRehypePlugins, defaultRemarkPlugins } from "streamdown";

import type { Rill } from "./core.ts";
import { defineEngine } from "./engine.ts";

export * from "./core.ts";
export * from "./engine.ts";

/** The prebuilt build: Streamdown's own remark and rehype plugins, in Rust. */
export const rillEngine = defineEngine({
  glue: () => import("../wasm/rill.js"),
  wasm: () => new URL("../wasm/rill_bg.wasm", import.meta.url),
  plugins: () => ({
    remarkPlugins: Object.values(defaultRemarkPlugins),
    rehypePlugins: Object.values(defaultRehypePlugins),
  }),
});

/** The prebuilt build's rill once `loadRill` or `initRill` has finished, else `null`. */
export function loadedRill(): Rill | null {
  return rillEngine.rill();
}

/** Fetches the prebuilt WebAssembly (Streamdown's pipeline) once and returns rill. */
export async function loadRill(): Promise<Rill> {
  await rillEngine.load();
  return rillEngine.rill()!;
}

/** Instantiates the prebuilt WebAssembly from its bytes (Node, SSR, tests). */
export async function initRill(wasm: BufferSource | WebAssembly.Module): Promise<Rill> {
  await rillEngine.init(wasm);
  return rillEngine.rill()!;
}
