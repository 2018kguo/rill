// rill's JavaScript core: wraps a rill WebAssembly export (the prebuilt one, or a build
// with your own Rust plugins) and decodes what it returns into HAST. Nothing here loads
// WebAssembly, so a custom build imports this and `streamdown.ts` only.
import { decodeHast, type HastNode } from "./decode.ts";
import { encodeLoneSurrogates, restoreLoneSurrogates } from "./surrogates.ts";

export { decodeHast, type HastNode } from "./decode.ts";

/**
 * How far rill took a document. `complete` is the final tree; `raw` (raw HTML, which
 * needs parse5) and `math` (KaTeX) trees still need their JavaScript tail, which
 * `rillPlugins` and `<Streamdown>` run for you.
 */
export type RillKind = "complete" | "raw" | "math";

/** The Streamdown `plugins` a render follows: `plugins.math` (`@streamdown/math` with
 * `singleDollarTextMath: false`) and `plugins.cjk` (`@streamdown/cjk`). */
export type Syntax = { math?: boolean; cjk?: boolean };

/** What a rill WebAssembly export returns (`rill::wasm::RenderedHast`). */
export type RenderedHast = {
  readonly kind: string;
  ops(): Uint32Array;
  strings(): string;
  free(): void;
};

export type Rill<A extends unknown[] = []> = {
  /**
   * The HAST for `source` with `syntax` (default: Streamdown with no `plugins`), or
   * `null` when it cannot cross into Rust (it holds lone surrogates and already uses
   * U+E000–E7FF, the range that carries them).
   */
  render(
    source: string,
    syntax?: Syntax,
    ...args: A
  ): { kind: RillKind; tree: HastNode } | null;
};

const KINDS = new Set<string>(["complete", "raw", "math"]);

function isRillKind(kind: string): kind is RillKind {
  return KINDS.has(kind);
}

/**
 * Wraps a WebAssembly export that returns `rill::wasm::render`'s result. It takes the
 * source, then `math` and `cjk` (for `rill::Options`); extra arguments pass through, so
 * a custom build can take per-render options.
 */
export function createRill<A extends unknown[]>(
  renderHast: (
    source: string,
    math: boolean,
    cjk: boolean,
    ...args: A
  ) => RenderedHast
): Rill<A> {
  return {
    render(raw, syntax = {}, ...args) {
      const encoded = encodeLoneSurrogates(raw);
      if (encoded === null) {
        return null;
      }
      // Rust replaces NUL like micromark; source slices then read the same string.
      const sliced = raw.includes("\0") ? raw.replaceAll("\0", "�") : raw;
      const transport = encoded.source.includes("\0")
        ? encoded.source.replaceAll("\0", "�")
        : encoded.source;
      const result = renderHast(
        transport,
        syntax.math === true,
        syntax.cjk === true,
        ...args
      );
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
