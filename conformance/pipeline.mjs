// Streamdown 1.6.11's default pipeline, which rill reproduces.
import { harden } from "rehype-harden";
import rehypeKatex from "rehype-katex";
import rehypeRaw from "rehype-raw";
import rehypeSanitize from "rehype-sanitize";
import remarkCjkFriendly from "remark-cjk-friendly";
import remarkCjkFriendlyGfmStrikethrough from "remark-cjk-friendly-gfm-strikethrough";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { unified } from "unified";

import { rehypeMathFragments } from "../js/src/math.ts";

export const remarkPlugins = [
  remarkGfm,
  [remarkMath, { singleDollarTextMath: false }],
  remarkCjkFriendly,
  remarkCjkFriendlyGfmStrikethrough,
];

const katex = [rehypeKatex, { errorColor: "var(--color-muted-foreground)" }];
const hardenOptions = [
  harden,
  {
    allowedImagePrefixes: ["*"],
    allowedLinkPrefixes: ["*"],
    allowedProtocols: ["*"],
    defaultOrigin: undefined,
    allowDataImages: true,
  },
];

export const rehypePlugins = [rehypeRaw, katex, rehypeSanitize, hardenOptions];

/** What runs on each math element of a `math` tree. */
export const mathPlugins = [katex, rehypeSanitize, hardenOptions];

export const processor = unified()
  .use(remarkParse)
  .use(remarkPlugins)
  .use(remarkRehype, { allowDangerousHtml: true })
  .use(rehypePlugins)
  .freeze();

/** What JavaScript still runs on rill's tree for each `kind`. */
export const tails = {
  complete: null,
  raw: unified().use(rehypePlugins).freeze(),
  // Everything but the math elements is final: render each one on its own.
  math: unified()
    .use(rehypeMathFragments(mathPlugins))
    .freeze(),
};
