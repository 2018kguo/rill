// rill's JavaScript core: wraps a rill build's WebAssembly (the prebuilt one, or a build
// with your own Rust plugins, both made by `rill::wasm_exports!`) and decodes what it
// returns into HAST. Nothing here loads WebAssembly (see `defineEngine`).
import { decodeHast, HastReader, type HastNode } from "./decode.ts";
import { Lru, noteKept } from "./prefix.ts";
import { encodeLoneSurrogates, restoreLoneSurrogates } from "./surrogates.ts";

export { decodeHast, HastReader, type HastNode } from "./decode.ts";

/**
 * How far rill took a document. `complete` is the final tree; `raw` (raw HTML, which
 * needs parse5) and `math` (KaTeX) trees still need their JavaScript tail, which
 * `rillPlugins` and `<Streamdown>` run for you.
 */
export type RillKind = "complete" | "raw" | "math";

/** The Streamdown `plugins` a render follows: `plugins.math` (`@streamdown/math` with
 * `singleDollarTextMath: false`) and `plugins.cjk` (`@streamdown/cjk`), on top of what the
 * build turns on itself; and the plugins of a page's own lists rill builds in (the
 * prebuilt build: see `knownLists` in streamdown.ts). */
export type Syntax = {
  math?: boolean;
  cjk?: boolean;
  /** remark-math's `singleDollarTextMath` (its default). */
  singleDollar?: boolean;
  /** remark-breaks. */
  breaks?: boolean;
  /** `false`: no remarkCodeMeta. */
  codeMeta?: boolean;
  /** rehype-sanitize with its own default schema (no options). */
  defaultSchema?: boolean;
  /** rehype-slug after the chain: its prefix, and whether it runs after rehype-katex. */
  slug?: { prefix: string; afterKatex: boolean };
  /** rehype-external-links after the chain, its options resolved. */
  externalLinks?: { target: string; rel: string[]; protocols: string[] };
};

/** `syntax` as the WebAssembly reads it (`rill::wasm::with_plugins`). */
export function syntaxBits(syntax: Syntax): number {
  return (
    (syntax.math === true ? 1 : 0) |
    (syntax.cjk === true ? 2 : 0) |
    (syntax.singleDollar === true ? 4 : 0) |
    (syntax.breaks === true ? 8 : 0) |
    (syntax.codeMeta === false ? 16 : 0) |
    (syntax.slug ? 32 : 0) |
    (syntax.slug?.afterKatex ? 64 : 0) |
    (syntax.externalLinks ? 128 : 0) |
    (syntax.defaultSchema === true ? 256 : 0)
  );
}

/** The strings `syntax`'s plugins take (`rill::wasm::with_plugins`). */
export function syntaxExtra(syntax: Syntax): string {
  const links = syntax.externalLinks;
  if (!(syntax.slug || links)) return "";
  return [
    syntax.slug?.prefix ?? "",
    links?.target ?? "",
    links?.rel.join(" ") ?? "",
    links?.protocols.join(" ") ?? "",
  ].join("\0");
}

/** A build's extra arguments (`engineArgs`): values `wasm-bindgen` takes and that
 * compare by value. */
export type Args = readonly (string | number | boolean | null | undefined)[];

/** What a rill WebAssembly export returns (`rill::wasm::RenderedHast`). */
export type RenderedHast = {
  readonly kind: string;
  ops(): Uint32Array;
  strings(): string;
  /** After an animated render: `[chars, cursor]` (see `AnimatePass`). */
  animated?(): Float64Array;
  free(): void;
};

/** The exports `rill::wasm_exports!` makes, from a build's `wasm-bindgen` glue. */
export type Glue<A extends Args = []> = {
  rillAbi(): number;
  renderHast(source: string, syntax: number, extra: string, ...args: A): RenderedHast;
  renderMarkup(
    source: string,
    syntax: number,
    extra: string,
    prefix: string,
    flags: number,
    overrides: string,
    animate: string,
    timing: Float64Array,
    ...args: A
  ): RenderedHast;
  pieces(...args: A): boolean;
  definitions(source: string, syntax: number, ...args: A): Uint32Array;
  splitsAt(source: string, syntax: number, at: number, ...args: A): boolean;
  remendStream(appended: string, reset: boolean): number;
  remendMiddle(): string;
};

/** The `rill::wasm::ABI` this package reads. */
export const ABI = 5;

/** rill for one set of a build's arguments. */
export type Rill = {
  /**
   * The HAST for `source` with `syntax` (default: Streamdown with no `plugins`), or
   * `null` when it cannot cross into Rust (it holds lone surrogates and already uses
   * U+E000–E7FF, the range that carries them) or a plugin leaves it to JavaScript.
   */
  render(source: string, syntax?: Syntax): { kind: RillKind; tree: HastNode } | null;
  /**
   * Streamdown's DOM for one block as segments (see crates/rill/src/markup.rs), read
   * with the returned reader; where only Streamdown renders it exactly, the reason (a
   * `FallbackReason`).
   */
  markup?: (source: string, syntax: Syntax, options: MarkupOptions) => HastReader | string;
  /** The link definitions at the root of `source`, as UTF-16 `[start, end)` ranges, or
   * `null` when one sits in a container or `source` has a footnote definition. Only
   * builds without plugins of their own (a plugin may read the whole text) have it. */
  definitions?: (source: string, syntax: Syntax) => [number, number][] | null;
  /** Whether no root-level block of `source` spans UTF-16 offset `at`. With
   * `definitions` only. */
  splitsAt?: (source: string, syntax: Syntax, at: number) => boolean;
  /** remend 1.4.0 with its default options (Streamdown's streaming repair), or `null`
   * for text it cannot take (lone surrogates). */
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
  /** JavaScript renders the build's math with KaTeX; without it, a block with math
   * renders through Streamdown. */
  katex?: boolean;
  /** The block ends in a code fence still streaming: one holding an element the page
   * gives its own component renders through Streamdown (its components read the fence's
   * state from a context only Streamdown's Block provides). */
  incomplete?: boolean;
  /** While `animated` streams: the animate plugin's pass on this block, whose results
   * the render writes back. */
  animate?: AnimatePass;
  /** Every element a React element (a block that was animated, settling as React
   * reconciles it). */
  deep?: boolean;
};

/** One block's pass of Streamdown's animate plugin (`rill::animate::Animate`). */
export type AnimatePass = {
  animation: string;
  /** The duration as the plugin writes it (`String(duration)`). */
  duration: string;
  easing: string;
  byChar: boolean;
  stagger: number;
  /** UTF-16 units of the block's text already shown. */
  shown: number;
  /** The timeline: its cursor, its clock, and its backlog. */
  cursor: number;
  now: number;
  backlog: number;
  /** Written by the render: the block's text length, and the cursor after it. */
  chars?: number;
  next?: number;
};

const NO_TIMING = new Float64Array(0);

/** A build's instance: its `Rill` for each set of arguments. */
export type RillBuild<A extends Args = []> = {
  /** The `Rill` for `args`: the same object for the same values. */
  bind(...args: A): Rill;
};

const KINDS = new Set<string>(["complete", "raw", "math"]);

/** What `remend.stream` returns for a whole text it leaves as it is (`u32::MAX`). */
const UNCHANGED = 0xffff_ffff;

function isRillKind(kind: string): kind is RillKind {
  return KINDS.has(kind);
}

/** `args` by value: the key rill caches by. */
export function argsKey(args: Args): string {
  return args.length === 0 ? "" : JSON.stringify(args);
}

/**
 * Wraps a build's glue (an instantiated module made by `rill::wasm_exports!`). Each
 * `bind(...args)` passes those arguments to every export, so a build can take
 * per-render options.
 */
export function createRill<A extends Args = []>(glue: Glue<A>): RillBuild<A> {
  const abi = glue.rillAbi?.();
  if (abi !== ABI) {
    throw new Error(
      `rill: this WebAssembly speaks boundary ${abi ?? "(none)"}, the package ${ABI}; ` +
        "rebuild it with the rill crate of the same version"
    );
  }
  // A trap (a stack overflow, a bug) leaves the instance unusable: its stack pointer
  // and allocator stay wherever the trap left them. From then on nothing calls into it,
  // and Streamdown renders everything in JavaScript.
  let trapped = false;
  const guard = <F extends (...args: never[]) => unknown>(run: F): F =>
    ((...args: never[]) => {
      if (trapped) throw new Error("rill: the WebAssembly trapped");
      try {
        return run(...args);
      } catch (error) {
        if (error instanceof WebAssembly.RuntimeError || error instanceof RangeError) {
          trapped = true;
        }
        throw error;
      }
    }) as F;
  const renderHast = guard(glue.renderHast.bind(glue));
  const renderMarkup = guard(glue.renderMarkup.bind(glue));
  const ranges = guard(glue.definitions.bind(glue));
  const splitsAt = guard(glue.splitsAt.bind(glue));
  const pieces = guard(glue.pieces.bind(glue));
  const remendStream = guard(glue.remendStream.bind(glue));
  const remendMiddle = guard(glue.remendMiddle.bind(glue));

  // remend does not depend on the arguments, and there is one stream in Rust: the text
  // it last healed, which it keeps (a text that extends it sends only what was
  // appended), and what it healed it into (an update sends only the healed text that
  // changed).
  let last: string | null = null;
  let lastHealed = "";
  // Texts healed whole (a message that does not stream, mounted again or repeated).
  const healedWhole = new Lru<string, string>(64);
  const heal = (text: string): string | null => {
    // A slice compared with === is V8's bulk comparison; startsWith goes unit by unit.
    const appended =
      last !== null && text.length >= last.length && text.slice(0, last.length) === last
        ? text.slice(last.length)
        : null;
    if (appended === null) {
      const known = healedWhole.get(text);
      if (known !== undefined) return known;
    }
    // The text so far was well formed (it ends on no lone half of a pair), so the
    // text is well formed when what it appends is.
    if (!(appended ?? text).isWellFormed()) {
      last = null;
      return null;
    }
    // Unknown until this call returns (it may throw).
    last = null;
    const keep = remendStream(appended ?? text, appended === null);
    // A whole text with nothing to heal (the usual finished answer) is itself.
    const unchanged = keep === UNCHANGED;
    // Appending keeps the last healed text whole (no copy of it).
    const middle = unchanged ? "" : remendMiddle();
    const healed = unchanged
      ? text
      : keep === lastHealed.length
        ? lastHealed + middle
        : lastHealed.slice(0, keep) + middle;
    noteKept(lastHealed, healed, unchanged ? 0 : keep);
    last = text;
    lastHealed = healed;
    if (appended === null) healedWhole.set(text, healed);
    return healed;
  };
  const remend = (text: string) => {
    try {
      return heal(text);
    } catch {
      last = null;
      return null;
    }
  };

  const bound = new Lru<string, Rill>(256);

  function bind(...args: A): Rill {
    const key = argsKey(args);
    const known = bound.get(key);
    if (known) return known;
    // Blocks rendered when not streaming (a message mounted again, or repeated).
    const rendered = new Lru<string, Map<string, [Uint32Array, string] | string>>(512);
    const markup = (source: string, syntax: Syntax, options: MarkupOptions) => {
      // Lone surrogates and NUL take the exact path (Streamdown's own Block).
      if (!source.isWellFormed() || source.includes("\0")) {
        return "surrogates";
      }
      const flags =
        (options.linkSafety ? 1 : 0) |
        (options.ssr ? 2 : 0) |
        (options.spine ? 4 : 0) |
        (options.deep ? 128 : 0) |
        (options.katex ? 8 : 0) |
        (options.incomplete ? 16 : 0);
      const bits = syntaxBits(syntax);
      const extra = syntaxExtra(syntax);
      const flagsKey = `${flags} ${bits} ${extra} ${options.prefix} ${options.overrides}`;
      const animate = options.animate;
      // Streaming and animated renders are not kept: they do not repeat.
      const keep = !(options.spine || animate);
      const known = keep ? rendered.get(source) : undefined;
      let output = known?.get(flagsKey);
      if (output === undefined) {
        const result = renderMarkup(
          source,
          bits,
          extra,
          options.prefix,
          (flags & 31) | (options.deep ? 32 : 0),
          options.overrides,
          animate ? `${animate.animation}\0${animate.duration}\0${animate.easing}` : "",
          animate
            ? Float64Array.of(
                animate.byChar ? 1 : 0,
                animate.stagger,
                animate.shown,
                animate.cursor,
                animate.now,
                animate.backlog
              )
            : NO_TIMING,
          ...args
        );
        try {
          output = result.kind === "markup" ? [result.ops(), result.strings()] : result.kind;
          if (animate && result.kind === "markup") {
            const animated = result.animated!();
            animate.chars = animated[0]!;
            animate.next = animated[1]!;
          }
        } finally {
          result.free();
        }
        if (keep) {
          const byKey = known ?? new Map();
          byKey.set(flagsKey, output);
          if (!known) rendered.set(source, byKey);
        }
      }
      return typeof output === "string" ? output : new HastReader(output[0], output[1], source);
    };
    const rill: Rill = {
      markup,
      remend,
      render(raw, syntax = {}) {
        const encoded = encodeLoneSurrogates(raw);
        if (encoded === null) {
          return null;
        }
        // Rust replaces NUL like micromark; source slices then read the same string.
        const sliced = raw.includes("\0") ? raw.replaceAll("\0", "�") : raw;
        const transport = encoded.source.includes("\0")
          ? encoded.source.replaceAll("\0", "�")
          : encoded.source;
        const result = renderHast(transport, syntaxBits(syntax), syntaxExtra(syntax), ...args);
        try {
          const kind = result.kind;
          if (kind === "unsupported") return null;
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
    if (pieces(...args)) {
      rill.definitions = (source: string, syntax: Syntax) => {
        if (!source.isWellFormed() || source.includes("\0")) return null;
        let flat: Uint32Array;
        try {
          flat = ranges(source, syntaxBits(syntax), ...args);
        } catch {
          return null;
        }
        if (flat.length === 1) return null;
        const out: [number, number][] = [];
        for (let index = 0; index < flat.length; index += 2) {
          out.push([flat[index]!, flat[index + 1]!]);
        }
        return out;
      };
      rill.splitsAt = (source: string, syntax: Syntax, at: number) => {
        if (!source.isWellFormed() || source.includes("\0")) return false;
        try {
          return splitsAt(source, syntaxBits(syntax), at, ...args);
        } catch {
          return false;
        }
      };
    }
    bound.set(key, rill);
    return rill;
  }

  return { bind };
}
