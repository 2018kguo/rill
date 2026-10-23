// rill-highlight (@2018kguo/rill-highlight) in place of Shiki for Streamdown's stock
// code plugin (`@streamdown/code`): Shiki's TextMate tokenizer in Rust, with the same
// grammars, themes and regex translations, giving the same tokens. Results follow the
// plugin's contract: a cached result now, or `null` and the callback later; once a
// language is loaded, results come back at once. A language whose grammar does not
// compile, custom themes or a failed load: Shiki as before. highlight.ts runs this in a
// worker.
import { createHighlightCore } from "@2018kguo/rill-highlight/core";
import { data } from "@2018kguo/rill-highlight/data";
import { createCodePlugin } from "@2018kguo/rill-highlight/streamdown";
import type { PluginConfig } from "streamdown";

type CodePlugin = NonNullable<PluginConfig["code"]>;
type Options = Parameters<CodePlugin["highlight"]>[0];
type Result = NonNullable<ReturnType<CodePlugin["highlight"]>>;
type Callback = (result: Result) => void;

/** `plugins.code.highlight`'s contract. */
export type Highlighter = {
  highlight(options: Options, callback: Callback): Result | null;
};

export type { Callback, CodePlugin, Options, Result };

type RillPlugin = {
  highlightSync(options: Options): Result | null;
  load(options: Pick<Options, "language" | "themes">): Promise<void>;
};

/** Results rill-highlight and Shiki returned (diagnostics, for tests and benchmarks). */
export const highlightCounts = { rill: 0, shiki: 0 };

let wasmSource: BufferSource | URL | Response | undefined;

/** Where rill-highlight's WebAssembly comes from (default: where its package finds it,
 * `dist/rill_highlight.wasm` next to its modules). */
export function setRillHighlightWasm(source: BufferSource | URL | Response) {
  wasmSource = source;
}

let plugin: RillPlugin | undefined;

function rillPlugin(): RillPlugin {
  plugin ??= createCodePlugin(
    wasmSource === undefined ? {} : { core: createHighlightCore({ wasm: wasmSource, data }) }
  ) as unknown as RillPlugin;
  return plugin;
}

/** Loads rill-highlight and a theme pair now, ahead of the first request. */
export function preloadRill(themes: [string, string]) {
  rillPlugin()
    .load({ language: "text", themes } as never)
    .catch((error: unknown) => {
      console.warn("rill: rill-highlight did not load, so Shiki highlights.", error);
    });
}

/** `stock.highlight` with rill-highlight wherever it gives Shiki's tokens. */
export function withRill(stock: Pick<CodePlugin, "highlight">): Highlighter {
  // Languages and themes (as given) left to Shiki: they did not load (a grammar that
  // does not compile, a theme rill-highlight does not bundle, no WebAssembly).
  const shiki = new Set<string>();
  const keyOf = (options: Options) => `${options.language}\0${options.themes.join("\0")}`;
  const viaShiki = (options: Options, callback: Callback) => {
    const counted = (result: Result) => {
      highlightCounts.shiki += 1;
      callback(result);
    };
    const result = stock.highlight(options, counted);
    if (result) highlightCounts.shiki += 1;
    return result;
  };
  const fallback = (options: Options, callback: Callback) => {
    shiki.add(keyOf(options));
    const result = viaShiki(options, callback);
    if (result) callback(result);
  };
  const named = (options: Options) =>
    (options.themes as unknown[]).every((theme) => typeof theme === "string");
  return {
    highlight(options, callback) {
      if (!named(options) || shiki.has(keyOf(options))) {
        return viaShiki(options, callback);
      }
      const rill = rillPlugin();
      try {
        const result = rill.highlightSync(options);
        if (result) {
          highlightCounts.rill += 1;
          return result;
        }
      } catch {
        shiki.add(keyOf(options));
        return viaShiki(options, callback);
      }
      rill.load(options).then(
        () => {
          try {
            const result = rill.highlightSync(options);
            if (result) {
              highlightCounts.rill += 1;
              callback(result);
            } else {
              fallback(options, callback);
            }
          } catch {
            fallback(options, callback);
          }
        },
        () => fallback(options, callback)
      );
      return null;
    },
  };
}
