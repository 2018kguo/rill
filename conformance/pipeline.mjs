// Streamdown 2.7.1's pipeline, which rill reproduces, built from the published packages
// in the order streamdown/index.tsx merges them (`mergedRemarkPlugins`,
// `mergedRehypePlugins`) for each plugin configuration:
//
//   remark: cjk.remarkPluginsBefore, gfm, codeMeta, cjk.remarkPluginsAfter, math.remarkPlugin
//   rehype: raw, sanitize, harden, math.rehypePlugin
import { cjk } from "@streamdown/cjk";
import { createMathPlugin, math } from "@streamdown/math";
import { defaultSchema } from "hast-util-sanitize";
import { harden } from "rehype-harden";
import rehypeExternalLinks from "rehype-external-links";
import rehypeKatex from "rehype-katex";
import rehypeRaw from "rehype-raw";
import rehypeSanitize from "rehype-sanitize";
import rehypeSlug from "rehype-slug";
import remarkBreaks from "remark-breaks";
import remarkCjkFriendly from "remark-cjk-friendly-1";
import remarkCjkFriendlyGfmStrikethrough from "remark-cjk-friendly-gfm-strikethrough-1";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { defaultRehypePlugins, defaultRemarkPlugins } from "streamdown";
import { unified } from "unified";

import { rehypeMathFragments } from "../js/src/math.ts";


/**
 * Streamdown 1.x's default plugins, as pages that pass their own lists to 2.7.1 often
 * still do (rill's `legacy_cjk`, `code_meta: false`, `Schema::Default`): remark-gfm,
 * remark-math, remark-cjk-friendly 1.2.3 after them; hast-util-sanitize's default schema
 * keeping remark-math's classes, KaTeX, then harden with blocked links as text.
 */
export const CLASSIC = {
  remarkPlugins: [
    [remarkGfm, {}],
    [remarkMath, { singleDollarTextMath: false }],
    remarkCjkFriendly,
    remarkCjkFriendlyGfmStrikethrough,
  ],
  rehypePlugins: [
    rehypeRaw,
    [
      rehypeSanitize,
      {
        ...defaultSchema,
        attributes: {
          ...defaultSchema.attributes,
          code: [["className", /^language-./, "math", "math-display", "math-inline"]],
        },
      },
    ],
    [rehypeKatex, { errorColor: "var(--color-muted-foreground)" }],
    [
      harden,
      {
        allowedImagePrefixes: ["*"],
        allowedLinkPrefixes: ["*"],
        allowedProtocols: ["*"],
        defaultOrigin: undefined,
        allowDataImages: true,
        linkBlockPolicy: "text-only",
      },
    ],
  ],
};

/** remark-math with its default `singleDollarTextMath: true`. */
const math1 = createMathPlugin({ singleDollarTextMath: true });

/** Plugins pages add to Streamdown's own (rill builds them in), after its lists. */
const EXTRAS = {
  breaks: { remark: [remarkBreaks], flags: ["--breaks"] },
  slug: { rehype: [rehypeSlug], flags: ["--slug"] },
  external: { rehype: [[rehypeExternalLinks, { target: "_blank" }]], flags: ["--external"] },
};

/** Streamdown's `plugins` prop for each configuration the harness checks, and plugins
 * added to its lists (`extra`). */
export const CONFIGS = {
  default: {},
  math: { math },
  cjk: { cjk },
  "math+cjk": { math, cjk },
  classic: CLASSIC,
  math1: { math: math1 },
  breaks: { extra: [EXTRAS.breaks] },
  slug: { extra: [EXTRAS.slug] },
  external: { extra: [EXTRAS.external] },
  extras: { math: math1, cjk, extra: [EXTRAS.breaks, EXTRAS.slug, EXTRAS.external] },
};

/** The remark and rehype plugin lists `<Streamdown plugins={plugins}>` runs. */
export function streamdownPlugins(plugins = {}) {
  if (plugins === CLASSIC) return CLASSIC;
  const extra = plugins.extra ?? [];
  return {
    remarkPlugins: [
      ...(plugins.cjk?.remarkPluginsBefore ?? []),
      ...Object.values(defaultRemarkPlugins),
      ...extra.flatMap((added) => added.remark ?? []),
      ...(plugins.cjk?.remarkPluginsAfter ?? []),
      ...(plugins.math ? [plugins.math.remarkPlugin] : []),
    ],
    rehypePlugins: [
      ...Object.values(defaultRehypePlugins),
      ...extra.flatMap((added) => added.rehype ?? []),
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
  const classic = plugins === CLASSIC;
  const katex = classic ? CLASSIC.rehypePlugins[2] : plugins.math?.rehypePlugin;
  /** What rill reports for this configuration's syntax (`stdio` flags). */
  const flags = classic
    ? ["--classic"]
    : [
        ...(plugins.math ? ["--math"] : []),
        ...(plugins.math === math1 ? ["--single-dollar"] : []),
        ...(plugins.cjk ? ["--cjk"] : []),
        ...(plugins.extra ?? []).flatMap((added) => added.flags),
      ];
  /** What JavaScript still runs on rill's tree for each `kind`. */
  const tails = {
    complete: null,
    raw: unified().use(rehypePlugins).freeze(),
    // Everything but the math elements is final: KaTeX renders each one on its own.
    ...(katex && {
      math: unified().use(rehypeMathFragments([katex])).freeze(),
    }),
  };
  return { plugins, processor, tails, flags, mathPlugins: katex ? [katex] : [] };
}

export const { processor, tails } = pipeline("default");
