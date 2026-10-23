// rill's JavaScript core: wraps a rill WebAssembly export (the prebuilt one, or a build
// with your own Rust plugins) and decodes what it returns into HAST. Nothing here loads
// WebAssembly, so a custom build imports this and `streamdown.ts` only.
import { decodeHast, HastReader, type HastNode } from "./decode.ts";
import { noteKept } from "./prefix.ts";
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
  /** The link definitions at the root of `source`, as UTF-16 `[start, end)` ranges, or
   * `null` when one sits in a container or `source` has a footnote definition. Builds
   * with the export only. */
  definitions?: (source: string, syntax: Syntax) => [number, number][] | null;
  /** Whether no root-level block of `source` spans UTF-16 offset `at`. Builds with the
   * export only. */
  splitsAt?: (source: string, syntax: Syntax, at: number) => boolean;
  /** remend 1.4.0 with its default options (Streamdown's streaming repair), or `null`
   * for text it cannot take (lone surrogates). Builds with the export only. */
  remend?: (text: string) => string | null;
};

/** The `<Streamdown>` props the markup depends on (`rill::MarkupOptions`). */
export type MarkupOptions = {
  prefix: string;
  linkSafety: boolean;
  /** React's server format (`renderToString`, and hydrating it). */
  ssr: boolean;
  /** Tag names the page gives components of its own. */
  overrides: string;
  /** While streaming: React elements down the block's last element (`rill::MarkupOptions::spine`). */
  spine?: boolean;
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
  renderMarkup?: RenderMarkup,
  definitions?: {
    ranges(source: string, math: boolean, cjk: boolean): Uint32Array;
    splitsAt(source: string, math: boolean, cjk: boolean, at: number): boolean;
  },
  remend?: {
    /** Heals the text so far plus `appended` (or just `appended`, with `reset`):
     * how many units of the last healed text the new one keeps, then `middle()`. */
    stream(appended: string, reset: boolean): number;
    middle(): string;
  }
): Rill<A> {
  // The text remend last healed in Rust, which keeps it (a text that extends it sends
  // only what was appended), and what it healed it into (an update sends only the
  // healed text that changed).
  let last: string | null = null;
  let lastHealed = "";
  const heal = remend
    ? (text: string) => {
        const appended =
          last !== null && text.startsWith(last) ? text.slice(last.length) : null;
        // Unknown until this call returns (it may throw).
        last = null;
        const keep = remend.stream(appended ?? text, appended === null);
        const healed = lastHealed.slice(0, keep) + remend.middle();
        noteKept(lastHealed, healed, keep);
        last = text;
        lastHealed = healed;
        return healed;
      }
    : undefined;
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
          (options.linkSafety ? 1 : 0) | (options.ssr ? 2 : 0) | (options.spine ? 4 : 0),
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
    ...(definitions
      ? {
          definitions: (source: string, syntax: Syntax) => {
            if (!source.isWellFormed() || source.includes("\0")) return null;
            const flat = definitions.ranges(source, syntax.math === true, syntax.cjk === true);
            if (flat.length === 1) return null;
            const ranges: [number, number][] = [];
            for (let index = 0; index < flat.length; index += 2) {
              ranges.push([flat[index]!, flat[index + 1]!]);
            }
            return ranges;
          },
          splitsAt: (source: string, syntax: Syntax, at: number) =>
            source.isWellFormed() &&
            !source.includes("\0") &&
            definitions.splitsAt(source, syntax.math === true, syntax.cjk === true, at),
        }
      : {}),
    ...(heal
      ? {
          remend: (text: string) => {
            if (!text.isWellFormed()) {
              last = null;
              return null;
            }
            try {
              return heal(text);
            } catch {
              last = null;
              return null;
            }
          },
        }
      : {}),
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
