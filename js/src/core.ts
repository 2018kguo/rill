// rill's JavaScript core: wraps a rill WebAssembly export (the prebuilt one, or a build
// with your own Rust plugins) and decodes what it returns into HAST. Nothing here loads
// WebAssembly, so a custom build imports this and `streamdown.ts` only.
import { decodeHast, HastReader, type HastNode } from "./decode.ts";
import { encodeLoneSurrogates, restoreLoneSurrogates } from "./surrogates.ts";

export { decodeHast, HastReader, type HastNode } from "./decode.ts";

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
  /**
   * Streamdown's DOM for one block as segments (see crates/rill/src/markup.rs), read
   * with the returned reader; `null` where only Streamdown renders it exactly. Only
   * builds with a markup export have it (the prebuilt one does).
   */
  markup?: (
    source: string,
    syntax: Syntax,
    options: MarkupOptions
  ) => HastReader | null;
};

/** The `<Streamdown>` props the markup depends on (`rill::MarkupOptions`). */
export type MarkupOptions = {
  prefix: string;
  linkSafety: boolean;
  /** React's server format (`renderToString`, and hydrating it). */
  ssr: boolean;
  /** Tag names the page gives components of its own. */
  overrides: string;
};

/** What a markup export takes: `rill::wasm::render_markup`'s arguments. */
export type RenderMarkup = (
  source: string,
  math: boolean,
  cjk: boolean,
  prefix: string,
  flags: number,
  overrides: string
) => RenderedHast;

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
  ) => RenderedHast,
  renderMarkup?: RenderMarkup
): Rill<A> {
  const markup = renderMarkup
    ? (source: string, syntax: Syntax, options: MarkupOptions) => {
        // Lone surrogates and NUL take the exact path (Streamdown's own Block).
        if (!source.isWellFormed() || source.includes("\0")) {
          return null;
        }
        const result = renderMarkup(
          source,
          syntax.math === true,
          syntax.cjk === true,
          options.prefix,
          (options.linkSafety ? 1 : 0) | (options.ssr ? 2 : 0),
          options.overrides
        );
        try {
          if (result.kind !== "markup") {
            return null;
          }
          return new HastReader(result.ops(), result.strings(), source);
        } finally {
          result.free();
        }
      }
    : undefined;
  return {
    ...(markup ? { markup } : {}),
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
