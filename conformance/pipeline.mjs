// Streamdown 2.7.1's pipeline, which rill reproduces, built from the published packages
// in the order streamdown/index.tsx merges them (`mergedRemarkPlugins`,
// `mergedRehypePlugins`) for each plugin configuration:
//
//   remark: cjk.remarkPluginsBefore, gfm, codeMeta, cjk.remarkPluginsAfter, math.remarkPlugin
//   rehype: raw, sanitize, harden, math.rehypePlugin
import { cjk } from "@streamdown/cjk";
import { math } from "@streamdown/math";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { defaultRehypePlugins, defaultRemarkPlugins } from "streamdown";
import { unified } from "unified";

import { rehypeMathFragments } from "../js/src/math.ts";

/** Streamdown's `plugins` prop for each configuration the harness checks. */
export const CONFIGS = {
  default: {},
  math: { math },
  cjk: { cjk },
  "math+cjk": { math, cjk },
};

/** The remark and rehype plugin lists `<Streamdown plugins={plugins}>` runs. */
export function streamdownPlugins(plugins = {}) {
  return {
    remarkPlugins: [
      ...(plugins.cjk?.remarkPluginsBefore ?? []),
      ...Object.values(defaultRemarkPlugins),
      ...(plugins.cjk?.remarkPluginsAfter ?? []),
      ...(plugins.math ? [plugins.math.remarkPlugin] : []),
    ],
    rehypePlugins: [
      ...Object.values(defaultRehypePlugins),
      ...(plugins.math ? [plugins.math.rehypePlugin] : []),
    ],
  };
}

/** Streamdown's processor and rill's JavaScript tails for configuration `name`. */
export function pipeline(name = "default") {
  const plugins = CONFIGS[name];
  if (!plugins) throw new Error(`Unknown configuration ${name}: ${Object.keys(CONFIGS)}`);
  const { remarkPlugins, rehypePlugins } = streamdownPlugins(plugins);
  const processor = unified()
    .use(remarkParse)
    .use(remarkPlugins)
    .use(remarkRehype, { allowDangerousHtml: true })
    .use(rehypePlugins)
    .freeze();
  /** What rill reports for this configuration's syntax (`stdio` flags). */
  const flags = [...(plugins.math ? ["--math"] : []), ...(plugins.cjk ? ["--cjk"] : [])];
  /** What JavaScript still runs on rill's tree for each `kind`. */
  const tails = {
    complete: null,
    raw: unified().use(rehypePlugins).freeze(),
    // Everything but the math elements is final: KaTeX renders each one on its own.
    ...(plugins.math && {
      math: unified().use(rehypeMathFragments([plugins.math.rehypePlugin])).freeze(),
    }),
  };
  return { plugins, processor, tails, flags, mathPlugins: plugins.math ? [plugins.math.rehypePlugin] : [] };
}

export const { processor, tails } = pipeline("default");
