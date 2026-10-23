// rill-highlight in place of Shiki for Streamdown's stock code plugin
// (`@streamdown/code`): Shiki's TextMate tokenizer in Rust, with the same grammars,
// themes and regex translations, giving the same tokens (crates/rill-highlight;
// highlight/test checks it against Shiki). Results follow the plugin's contract: a
// cached result now, or `null` and the callback later; once a language is loaded,
// results come back at once. A language whose grammar does not compile, custom themes
// or a failed load: Shiki as before. highlight.ts runs this in a worker.
import type { PluginConfig } from "streamdown";

import { createCodePlugin } from "../highlight/code-plugin.js";
import { createHighlightCore } from "../highlight/core.js";
import { grammars, languages, themes } from "../highlight/index.js";

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
  highlight(options: Options, callback?: Callback): Result | null;
  highlightSync(options: Options): Result | null;
  load(options: Pick<Options, "language" | "themes">): Promise<void>;
};

/** Results rill-highlight and Shiki returned (diagnostics, for tests and benchmarks). */
export const highlightCounts = { rill: 0, shiki: 0 };

let wasmSource: BufferSource | URL | Response | undefined;

/** Where rill-highlight's WebAssembly comes from (default: the package's
 * `wasm/rill_highlight.wasm`). */
export function setRillHighlightWasm(source: BufferSource | URL | Response) {
  wasmSource = source;
}

let loading: Promise<RillPlugin | null> | undefined;
let loaded: RillPlugin | null = null;

function rillPlugin(): Promise<RillPlugin | null> {
  loading ??= createHighlightCore({
    wasm: wasmSource ?? new URL("../wasm/rill_highlight.wasm", import.meta.url),
    data: {
      languages,
      grammar: (name: string) => grammars[name]!().then((module) => module.default),
      theme: (name: string) => themes[name]!().then((module) => module.default),
    },
  }).then(
    (core: unknown) => {
      loaded = createCodePlugin({ core }) as RillPlugin;
      return loaded;
    },
    (error: unknown) => {
      console.warn("rill: rill-highlight did not load, so Shiki highlights.", error);
      return null;
    }
  );
  return loading;
}


/** Whether Shiki's options name only bundled themes rill-highlight has. */
function rillThemes(options: Options): boolean {
  const pair = options.themes as unknown[];
  return pair.every((theme) => typeof theme === "string" && theme in themes);
}

/** `stock.highlight` with rill-highlight wherever it gives Shiki's tokens. */
export function withRill(stock: Pick<CodePlugin, "highlight">): Highlighter {
  // Languages (as given) left to Shiki: their grammar did not compile.
  const shiki = new Set<string>();
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
    shiki.add(options.language);
    const result = viaShiki(options, callback);
    if (result) callback(result);
  };
  return {
    highlight(options, callback) {
      if (!rillThemes(options) || shiki.has(options.language)) {
        return viaShiki(options, callback);
      }
      const ready = loaded;
      if (ready) {
        try {
          const result = ready.highlightSync(options);
          if (result) {
            highlightCounts.rill += 1;
            return result;
          }
        } catch {
          shiki.add(options.language);
          return viaShiki(options, callback);
        }
      }
      rillPlugin().then((plugin) => {
        if (!plugin) {
          fallback(options, callback);
          return;
        }
        plugin.load(options).then(
          () => {
            try {
              const result = plugin.highlightSync(options);
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
      });
      return null;
    },
  };
}

