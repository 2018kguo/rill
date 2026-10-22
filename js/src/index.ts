// rill in JavaScript: wraps a rill WebAssembly export (the prebuilt one, or a build
// with your own Rust plugins) and decodes what it returns into HAST.
import { decodeHast, type HastNode } from "./decode.ts";
import { encodeLoneSurrogates, restoreLoneSurrogates } from "./surrogates.ts";

export { decodeHast, type HastNode } from "./decode.ts";

/**
 * How far rill took a document. `complete` is the final tree; `raw` (raw HTML, which
 * needs parse5) and `math` (KaTeX) trees still need their JavaScript tail, which
 * `rillPlugins` runs for you.
 */
export type RillKind = "complete" | "raw" | "math";

/** What a rill WebAssembly export returns (`rill::wasm::RenderedHast`). */
export type RenderedHast = {
  readonly kind: string;
  ops(): Uint32Array;
  strings(): string;
  free(): void;
};

export type Rill<A extends unknown[] = []> = {
  /**
   * The HAST for `source`, or `null` when it cannot cross into Rust (it holds lone
   * surrogates and already uses U+E000–E7FF, the range that carries them).
   */
  render(source: string, ...args: A): { kind: RillKind; tree: HastNode } | null;
};

const KINDS = new Set<string>(["complete", "raw", "math"]);

function isRillKind(kind: string): kind is RillKind {
  return KINDS.has(kind);
}

/**
 * Wraps a WebAssembly export that returns `rill::wasm::render`'s result. Extra
 * arguments pass through, so a custom build can take per-render options.
 */
export function createRill<A extends unknown[]>(
  renderHast: (source: string, ...args: A) => RenderedHast
): Rill<A> {
  return {
    render(raw, ...args) {
      const encoded = encodeLoneSurrogates(raw);
      if (encoded === null) {
        return null;
      }
      // Rust replaces NUL like micromark; source slices then read the same string.
      const sliced = raw.includes("\0") ? raw.replaceAll("\0", "�") : raw;
      const transport = encoded.source.includes("\0")
        ? encoded.source.replaceAll("\0", "�")
        : encoded.source;
      const result = renderHast(transport, ...args);
      try {
        const kind = result.kind;
        if (!isRillKind(kind)) {
          throw new Error(`Unknown rill kind ${kind}`);
        }
        const table = encoded.mapped
          ? restoreLoneSurrogates(result.strings())
          : result.strings();
        return { kind, tree: decodeHast(result.ops(), table, sliced) };
      } finally {
        result.free();
      }
    },
  };
}

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
