// The remark and rehype lists pages give `<Streamdown>` that the prebuilt build renders
// itself: Streamdown's own plugins (by identity), and the plugins pages most often add
// (remark-breaks, remark-math with either dollar rule, rehype-katex, rehype-slug,
// rehype-external-links), each recognized by its name or the shape of its code (names
// do not survive minifiers), then checked against rill's own implementation on a probe
// document: the plugin's output there must be rill's, or the list is not known.
import { defaultRehypePlugins, defaultRemarkPlugins } from "streamdown";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { unified, type Pluggable } from "unified";

import type { HastNode, Rill, Syntax } from "./core.ts";

/** What rill renders a page's lists with, and the entry that renders their math. */
export type Known = { syntax: Syntax; katex: Pluggable | undefined };

/** Why rill does not know a list: which list, and the plugins in it it does not know. */
export type Unknown = { reason: "remarkPlugins" | "rehypePlugins"; detail: string };

type Entry = { attacher: unknown; options: unknown; plugin: unknown };

const entryOf = (plugin: unknown): Entry =>
  Array.isArray(plugin)
    ? { attacher: plugin[0], options: plugin[1], plugin }
    : { attacher: plugin, options: undefined, plugin };

const nameOf = (attacher: unknown): string =>
  (typeof attacher === "function" && attacher.name) || "anonymous";

const GFM = entryOf(defaultRemarkPlugins.gfm);
const CODE_META = defaultRemarkPlugins.codeMeta;
const RAW = defaultRehypePlugins.raw;
const SANITIZE = entryOf(defaultRehypePlugins.sanitize);
const HARDEN = entryOf(defaultRehypePlugins.harden);

const isPlainObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && Object.getPrototypeOf(value) === Object.prototype;

const empty = (options: unknown) =>
  options === undefined || (isPlainObject(options) && Object.keys(options).length === 0);

function sameValue(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true;
  if (Array.isArray(left) && Array.isArray(right)) {
    return left.length === right.length && left.every((item, index) => sameValue(item, right[index]));
  }
  if (isPlainObject(left) && isPlainObject(right)) {
    const keys = Object.keys(left).filter((key) => left[key] !== undefined);
    const other = Object.keys(right).filter((key) => right[key] !== undefined);
    return keys.length === other.length && keys.every((key) => sameValue(left[key], right[key]));
  }
  return false;
}

/** The plugin's source, and its transformer's (what minifiers keep: strings and
 * property names). */
function sources(attacher: unknown, options: unknown): string {
  if (typeof attacher !== "function") return "";
  let out = Function.prototype.toString.call(attacher);
  try {
    // An attacher only reads its options and returns its transformer (unified calls it
    // the same way); one that needs a processor throws, and is not one of these.
    const transformer = (attacher as (this: unknown, options: unknown) => unknown).call(
      unified(),
      options
    );
    if (typeof transformer === "function") out += Function.prototype.toString.call(transformer);
  } catch {}
  return out;
}

// ---------------------------------------------------------------------------------
// Probes: the plugin against rill's implementation.
// ---------------------------------------------------------------------------------

/** remark-breaks and remark-math. */
const REMARK_PROBE = `a
b\r\nc *d\re* [f
g](h) $x$ and $$y$$ \\$z$ a$b$c

- one
  two

| a | $q$ |
|---|---|
| 1 | 2 |

$$
m
$$
`;

/** rehype-slug and rehype-external-links. */
const REHYPE_PROBE = `# Hello, World!

## Hello, World!

### Ünïcode 中文 🚀 a_b-c \`code\` *em*

#### hello world-1

[a](https://x.com) [b](http://y.org/p) [c](//z.net) [d](mailto:a@b.co) [e](/rel)
[f](#frag) [g](ftp://f.example) <https://auto.example> www.w.example[^1]

[^1]: Note [h](https://n.example).
`;

const withoutPositions = (node: unknown): unknown =>
  JSON.stringify(node, (key, value) => (key === "position" || key === "data" ? undefined : value));

/** Streamdown's pipeline with `remark` in its remark list, as HAST. */
function streamdownTree(source: string, remark: Pluggable[], rehype: Pluggable[]): unknown {
  const processor = unified()
    .use(remarkParse)
    .use(remark)
    .use(remarkRehype, { allowDangerousHtml: true })
    .use(rehype);
  return processor.runSync(processor.parse(source), source);
}

/** Whether `plugin` (a remark plugin) renders the probe as rill does with `syntax`. */
function remarkProbe(rill: Rill, plugin: Pluggable, syntax: Syntax): boolean {
  try {
    const ours = rill.render(REMARK_PROBE, syntax);
    if (!ours || ours.kind === "raw") return false;
    const theirs = streamdownTree(
      REMARK_PROBE,
      [defaultRemarkPlugins.gfm!, CODE_META!, plugin],
      Object.values(defaultRehypePlugins)
    );
    return withoutPositions(ours.tree) === withoutPositions(theirs);
  } catch {
    return false;
  }
}

/** Whether `plugin` (a rehype plugin after Streamdown's) changes the probe as rill does
 * with `syntax`. */
function rehypeProbe(rill: Rill, plugin: Pluggable, syntax: Syntax): boolean {
  try {
    const ours = rill.render(REHYPE_PROBE, syntax);
    const before = rill.render(REHYPE_PROBE, {});
    if (!(ours && before) || ours.kind !== "complete" || before.kind !== "complete") return false;
    // A list holding the entry: `use([attacher, options])` would read it as a list.
    const processor = unified().use([plugin] as never);
    const theirs = processor.runSync(structuredClone(before.tree) as never);
    // It must do something on the probe, and what rill does.
    return (
      withoutPositions(theirs) !== withoutPositions(before.tree) &&
      withoutPositions(theirs) === withoutPositions(ours.tree)
    );
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------------
// Recognizing the lists.
// ---------------------------------------------------------------------------------

const looksLike = {
  breaks: (name: string, source: string) =>
    name === "remarkBreaks" ||
    /^function\s*[\w$]*\(\)\s*\{\s*return\s+function\s*[\w$]*\(\s*([\w$]+)\s*\)\s*\{\s*[\w$.]+\(\s*\1\s*\)\s*;?\s*\}\s*;?\s*\}/.test(
      source.replace(/\s+/g, " ")
    ),
  // remark-math only adds syntax (as any syntax plugin does): the probe tells.
  math: (name: string, source: string) =>
    name === "remarkMath" || (source.includes("micromarkExtensions") && source.includes("fromMarkdownExtensions")),
  katex: (name: string, source: string) =>
    name === "rehypeKatex" || (source.includes("language-math") && source.includes("math-display")),
  slug: (name: string, source: string) =>
    name === "rehypeSlug" || (source.includes(".reset()") && source.includes(".slug(") && source.includes("properties.id")),
  external: (name: string, source: string) =>
    name === "rehypeExternalLinks" || (source.includes("properties.href") && /["'`]\/\/["'`]/.test(source)),
};

const ids = new WeakMap<object, number>();
let nextId = 0;

/** A key for `value` by what it is: JSON for plain options (a page often writes them
 * inline, a new object each render), else its identity. */
function keyOf(value: unknown): string {
  if (value === undefined) return "u";
  if ((typeof value === "object" && value !== null) || typeof value === "function") {
    if (isPlainObject(value) || Array.isArray(value)) {
      try {
        const json = JSON.stringify(value, (_, inner) => {
          if (typeof inner === "function" || (typeof inner === "object" && inner !== null && !isPlainObject(inner) && !Array.isArray(inner))) {
            throw new Error("not plain");
          }
          return inner;
        });
        if (json !== undefined) return `j${json}`;
      } catch {}
    }
    let id = ids.get(value);
    if (id === undefined) {
      id = nextId++;
      ids.set(value, id);
    }
    return `#${id}`;
  }
  return JSON.stringify(value) ?? String(value);
}

const listKey = (list: readonly unknown[]) =>
  list
    .map((plugin) => {
      const entry = entryOf(plugin);
      return Array.isArray(plugin) ? `${keyOf(entry.attacher)}:${keyOf(entry.options)}` : keyOf(plugin);
    })
    .join(",");

const known = new WeakMap<Rill, Map<string, Known | Unknown>>();

/**
 * How the prebuilt build renders a page's `remarkPlugins` and `rehypePlugins` (each
 * Streamdown's own when not given), or why it does not. `math`: `plugins.math` is set
 * too (Streamdown adds remark-math and rehype-katex after the lists). Cached by the
 * lists' plugins and options, so lists written inline probe once.
 */
export function knownLists(
  rill: Rill,
  remark: readonly unknown[],
  rehype: readonly unknown[],
  math: boolean
): Known | Unknown {
  let byKey = known.get(rill);
  if (!byKey) {
    byKey = new Map();
    known.set(rill, byKey);
  }
  const key = `${math ? 1 : 0}|${listKey(remark)}|${listKey(rehype)}`;
  let result = byKey.get(key);
  if (!result) {
    result = recognize(rill, remark, rehype, math);
    if (byKey.size >= 256) byKey.clear();
    byKey.set(key, result);
  }
  return result;
}

function recognize(
  rill: Rill,
  remark: readonly unknown[],
  rehype: readonly unknown[],
  streamdownMath: boolean
): Known | Unknown {
  const syntax: Syntax = { codeMeta: false };
  const unknownRemark: string[] = [];
  let gfm = false;
  let remarkMath: Pluggable | undefined;
  let breaks: Pluggable | undefined;
  for (const plugin of remark) {
    const entry = entryOf(plugin);
    if (entry.attacher === GFM.attacher && (plugin === defaultRemarkPlugins.gfm || empty(entry.options))) {
      if (gfm) unknownRemark.push(nameOf(entry.attacher));
      gfm = true;
      continue;
    }
    if (plugin === CODE_META && syntax.codeMeta === false) {
      delete syntax.codeMeta;
      continue;
    }
    const name = nameOf(entry.attacher);
    const source = sources(entry.attacher, entry.options);
    if (!breaks && empty(entry.options) && looksLike.breaks(name, source)) {
      breaks = plugin as Pluggable;
      continue;
    }
    if (!remarkMath && looksLike.math(name, source)) {
      const options = entry.options as { singleDollarTextMath?: unknown } | undefined;
      const keys = options === undefined ? [] : Object.keys(options);
      if (keys.every((key) => key === "singleDollarTextMath")) {
        remarkMath = plugin as Pluggable;
        syntax.singleDollar = options?.singleDollarTextMath !== false;
        continue;
      }
    }
    unknownRemark.push(name);
  }
  if (!gfm && unknownRemark.length === 0) unknownRemark.push("(no remark-gfm)");
  if (remarkMath && streamdownMath) unknownRemark.push(`${nameOf(entryOf(remarkMath).attacher)} with plugins.math`);
  if (unknownRemark.length > 0) return { reason: "remarkPlugins", detail: unknownRemark.join(", ") };

  // rehype: raw, sanitize, harden in Streamdown's order, then the plugins after the
  // chain (rehype-katex last of all, or before rehype-slug).
  const unknownRehype: string[] = [];
  const chain: string[] = [];
  let katex: Pluggable | undefined;
  let slug: Pluggable | undefined;
  let external: Pluggable | undefined;
  for (const plugin of rehype) {
    const entry = entryOf(plugin);
    if (plugin === RAW || entry.attacher === RAW) {
      if (chain.length !== 0 || !empty(entry.options)) unknownRehype.push(nameOf(entry.attacher));
      chain.push("raw");
      continue;
    }
    if (entry.attacher === SANITIZE.attacher) {
      if (chain.join() !== "raw") unknownRehype.push(nameOf(entry.attacher));
      else if (entry.options === undefined) syntax.defaultSchema = true;
      else if (entry.options !== SANITIZE.options) unknownRehype.push(`${nameOf(entry.attacher)} (its schema)`);
      chain.push("sanitize");
      continue;
    }
    if (entry.attacher === HARDEN.attacher) {
      if (chain.join() !== "raw,sanitize") unknownRehype.push(nameOf(entry.attacher));
      else if (entry.options !== HARDEN.options && !sameValue(entry.options, HARDEN.options)) {
        unknownRehype.push(`${nameOf(entry.attacher)} (its options)`);
      }
      chain.push("harden");
      continue;
    }
    const name = nameOf(entry.attacher);
    const after = chain.join() === "raw,sanitize,harden";
    const source = sources(entry.attacher, entry.options);
    if (after && !katex && looksLike.katex(name, source)) {
      katex = plugin as Pluggable;
      continue;
    }
    if (after && !slug && looksLike.slug(name, source)) {
      const options = entry.options as { prefix?: unknown } | undefined;
      const keys = options === undefined ? [] : Object.keys(options);
      if (keys.every((key) => key === "prefix") && (options?.prefix === undefined || typeof options.prefix === "string")) {
        syntax.slug = { prefix: (options?.prefix as string | undefined) || "", afterKatex: katex !== undefined };
        slug = plugin as Pluggable;
        continue;
      }
    }
    if (after && !external && looksLike.external(name, source)) {
      const links = externalLinksOptions(entry.options);
      // After rehype-katex it sees KaTeX's output, which holds links only with `trust`.
      const trusted = katex !== undefined && Boolean((entryOf(katex).options as { trust?: unknown })?.trust);
      if (links && !trusted) {
        syntax.externalLinks = links;
        external = plugin as Pluggable;
        continue;
      }
    }
    unknownRehype.push(name);
  }
  if (chain.join() !== "raw,sanitize,harden" && unknownRehype.length === 0) {
    unknownRehype.push(`(Streamdown's raw, sanitize and harden: ${chain.join(", ") || "none"})`);
  }
  if (remarkMath && !katex) unknownRehype.push("(remark-math without rehype-katex)");
  if (katex && !remarkMath) unknownRehype.push(`${nameOf(entryOf(katex).attacher)} without remark-math`);
  if (unknownRehype.length > 0) return { reason: "rehypePlugins", detail: unknownRehype.join(", ") };

  // Each plugin found by name or shape renders the probe as rill does.
  if (breaks) {
    if (!remarkProbe(rill, breaks, { breaks: true })) return { reason: "remarkPlugins", detail: nameOf(entryOf(breaks).attacher) };
    syntax.breaks = true;
  }
  if (remarkMath) {
    syntax.math = true;
    const probe = { math: true, singleDollar: syntax.singleDollar === true };
    if (!remarkProbe(rill, remarkMath, probe)) return { reason: "remarkPlugins", detail: nameOf(entryOf(remarkMath).attacher) };
  }
  if (slug && !rehypeProbe(rill, slug, { slug: { ...syntax.slug!, afterKatex: false } })) {
    return { reason: "rehypePlugins", detail: nameOf(entryOf(slug).attacher) };
  }
  if (external && !rehypeProbe(rill, external, { externalLinks: syntax.externalLinks! })) {
    return { reason: "rehypePlugins", detail: nameOf(entryOf(external).attacher) };
  }
  if (syntax.codeMeta !== false) delete syntax.codeMeta;
  return { syntax, katex };
}

/** rehype-external-links' options as rill takes them: `target`, `rel` and `protocols`
 * (others, such as `content` or `test`, are not). */
function externalLinksOptions(options: unknown): Syntax["externalLinks"] | null {
  if (options === undefined) return { target: "", rel: ["nofollow"], protocols: ["http", "https"] };
  if (!isPlainObject(options)) return null;
  for (const key of Object.keys(options)) {
    if (!["target", "rel", "protocols"].includes(key) && options[key] !== undefined) return null;
  }
  const { target, rel, protocols } = options as { target?: unknown; rel?: unknown; protocols?: unknown };
  if (target !== undefined && typeof target !== "string") return null;
  const words = (value: string) => value.trim().split(/[ \t\n\r\f]+/).filter(Boolean);
  // `rel || ["nofollow"]`: an empty string is the default too.
  const resolvedRel =
    rel === undefined || rel === "" ? ["nofollow"] : typeof rel === "string" ? words(rel) : rel;
  if (!(Array.isArray(resolvedRel) && resolvedRel.every((token) => typeof token === "string" && !/\s/.test(token)))) {
    return null;
  }
  const resolvedProtocols = protocols === undefined ? ["http", "https"] : protocols;
  if (!(Array.isArray(resolvedProtocols) && resolvedProtocols.every((p) => typeof p === "string" && p !== "" && !/\s/.test(p)))) {
    return null;
  }
  return { target: target ?? "", rel: resolvedRel as string[], protocols: resolvedProtocols as string[] };
}

export type { HastNode };
