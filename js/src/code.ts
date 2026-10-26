// `@2018kguo/rill/code`: `@streamdown/code`'s `code` and `createCodePlugin`, highlighting
// with rill-highlight (Shiki's tokenizer in Rust, the same tokens) instead of Shiki, so a
// page that imports it from here ships no Shiki.
//
//   import { code } from "@2018kguo/rill/code"; // in place of "@streamdown/code"
//
// rill highlights a stock code plugin's blocks in its worker, so the page itself needs
// only what Streamdown reads synchronously (the themes, the languages): rill-highlight's
// own plugin (`@2018kguo/rill-highlight/streamdown`, its manifest and loaders) loads on
// the first `highlight` call this plugin gets, which only a page without the worker
// makes.
import type { CodePluginOptions, CodeHighlighterPlugin as HighlightPlugin } from "@2018kguo/rill-highlight/streamdown";
import type { CodeHighlighterPlugin } from "streamdown";

import { LANGUAGES } from "./languages.ts";

let names: Map<string, string> | undefined;
/** Each language name and alias, to the language it names. */
const languageNames = () => {
  if (!names) {
    names = new Map();
    for (const entry of LANGUAGES.split(",")) {
      const [id, ...aliases] = entry.split(" ") as [string, ...string[]];
      names.set(id, id);
      for (const alias of aliases) names.set(alias, id);
    }
  }
  return names;
};

export function createCodePlugin(options: CodePluginOptions = {}): CodeHighlighterPlugin {
  const themes = options.themes ?? ["github-light", "github-dark"];
  let loaded: HighlightPlugin | undefined;
  let loading: Promise<HighlightPlugin> | undefined;
  const load = () =>
    (loading ??= import("@2018kguo/rill-highlight/streamdown").then(
      (module) => (loaded = module.createCodePlugin(options))
    ));
  return {
    name: "shiki",
    type: "code-highlighter",
    supportsLanguage: (language) => languageNames().has(language.trim().toLowerCase()),
    getSupportedLanguages: () => [...new Set(languageNames().values())] as never,
    getThemes: () => themes as never,
    highlight(request, callback) {
      if (loaded) return loaded.highlight(request as never, callback as never) as never;
      load().then(
        (plugin) => {
          const result = plugin.highlight(request as never, callback as never);
          if (result) callback?.(result as never);
        },
        (error: unknown) => console.error("[rill code] Failed to load rill-highlight:", error)
      );
      return null;
    },
  };
}

export const code = createCodePlugin();
