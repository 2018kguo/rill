"use client";
// `@2018kguo/rill/streamdown`: Streamdown 2.7.1 with rill rendering its blocks. Change
// `from "streamdown"` to `from "@2018kguo/rill/streamdown"`: everything Streamdown
// exports is here, and `Streamdown` and `Block` render the same DOM with much less
// work.
//
// `Streamdown` is Streamdown's own component (root, contexts, caret, remend, block
// splitting, memoization) with rill's `Block` as its `BlockComponent`. `Block` renders
// a block as Streamdown's components would, from segments Rust emits
// (crates/rill/src/markup.rs): static elements as one React element each with their
// markup as HTML, and React only where the DOM is interactive (code blocks, tables,
// images, link buttons). Whatever rill does not reproduce exactly (raw HTML parse5
// must see, an animation plugin, plugins or props it does not know, components of the
// page's own inside an incomplete block, the WebAssembly not loaded yet) renders with
// Streamdown's own `Block`, the same way.
import {
  cloneElement,
  createContext,
  createElement,
  Fragment,
  memo,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ComponentType,
  type MouseEvent,
  type ReactElement,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { jsx, jsxs } from "react/jsx-runtime";
import { toJsxRuntime } from "hast-util-to-jsx-runtime";
import {
  Block as StreamdownBlock,
  defaultComponents,
  defaultRehypePlugins,
  defaultRemarkPlugins,
  normalizeHtmlIndentation,
  Streamdown as StreamdownComponent,
  StreamdownContext,
  type BlockProps,
  type PluginConfig,
  type StreamdownProps,
} from "streamdown";
import type { Pluggable } from "unified";

import { parseMarkdownIntoBlocks, splitDefinitionBlock } from "./blocks.ts";
import { CodeSegment, shellLabels, type ShellLabels } from "./code-block.ts";
import type { HastNode, HastReader, MarkupOptions, Rill } from "./core.ts";
import { highlighterFor, type Highlighter } from "./highlight.ts";
import { createCn, hastToHtml, type Cn } from "./html.ts";
import { mathFragments } from "./math.ts";
import { DEFAULT_MAX_SOURCE_LENGTH, optionsOf } from "./plugins.ts";
import { load, pluginsFor, snapshot, subscribe } from "./react.ts";

export * from "streamdown";
export {
  DEFAULT_MAX_SOURCE_LENGTH,
  rillPlugins,
  type RenderKind,
  type RillPluginsOptions,
} from "./plugins.ts";
export { useRill } from "./react.ts";
// Streamdown's block splitter, without its whole-text tests on every streaming update.
export { parseMarkdownIntoBlocks } from "./blocks.ts";
export {
  highlightCounts,
  pendingHighlights,
  preloadHighlighting,
  setHighlightWasm,
  setRillHighlight,
} from "./highlight.ts";

/** What `Block` needs from `<Streamdown>`'s props that its own props lack. */
type Settings = {
  plugins: PluginConfig | undefined;
  cn: Cn;
  prefix: string;
  /** `fallbackComponent` is set: every element renders through Streamdown. */
  fallback: boolean;
  /** Streamdown hands the whole text to one Block, which renders its blocks itself (a
   * message that has not streamed: one component instead of one per block). */
  whole: boolean;
  highlight: Highlighter | undefined;
  /** The code blocks' control labels, or `null` when the page gives its own copy or
   * download icon (their controls then render through React from the start). */
  shell: ShellLabels | null;
};

const RillContext = createContext<Settings | null>(null);


/** How many block renders rill took and how many went to Streamdown's own Block
 * (diagnostics, for tests and benchmarks). */
export const blockRenders = { rill: 0, streamdown: 0, tableTails: 0 };

type Props = StreamdownProps & Record<string, unknown>;

/** Streamdown's memo comparison (index.tsx), so this re-renders exactly when it does. */
function sameStreamdownProps(previous: Props, next: Props): boolean {
  return (
    previous.children === next.children &&
    previous.shikiTheme === next.shikiTheme &&
    previous.isAnimating === next.isAnimating &&
    previous.animated === next.animated &&
    previous.mode === next.mode &&
    previous.plugins === next.plugins &&
    previous.className === next.className &&
    previous.linkSafety === next.linkSafety &&
    previous.lineNumbers === next.lineNumbers &&
    previous.codeBlockMaxHeight === next.codeBlockMaxHeight &&
    previous.tableMaxHeight === next.tableMaxHeight &&
    previous.normalizeHtmlIndentation === next.normalizeHtmlIndentation &&
    previous.literalTagContent === next.literalTagContent &&
    previous.disableAutolinkProtocols === next.disableAutolinkProtocols &&
    JSON.stringify(previous.translations) === JSON.stringify(next.translations) &&
    previous.prefix === next.prefix &&
    previous.dir === next.dir &&
    previous.fallbackComponent === next.fallbackComponent &&
    sameRootAttributes(previous, next)
  );
}

function rootAttributes(props: object): Record<string, unknown> {
  const attributes: Record<string, unknown> = {};
  for (const [name, value] of Object.entries(props)) {
    if (name.startsWith("data-") || name.startsWith("aria-")) {
      attributes[name] = value;
    }
  }
  return attributes;
}

function sameRootAttributes(previous: object, next: object): boolean {
  const before = rootAttributes(previous);
  const after = rootAttributes(next);
  const names = Object.keys(before);
  return (
    names.length === Object.keys(after).length &&
    names.every((name) => Object.hasOwn(after, name) && before[name] === after[name])
  );
}

/** Streamdown 2.7.1's `<Streamdown>`, rendering its blocks with rill. */
export const Streamdown = memo(function Streamdown(props: StreamdownProps) {
  const { plugins, prefix, fallbackComponent, translations, icons } = props;
  // A message that has streamed keeps a Block per block (their state, as Streamdown
  // keeps it); one that has not renders its blocks from one Block. Per-block props
  // (`dir`, animation, a page's own splitter or Block) keep Streamdown's blocks.
  const streamed = useRef(false);
  if (props.isAnimating) streamed.current = true;
  const whole =
    !streamed.current &&
    props.mode !== "static" &&
    props.dir === undefined &&
    !props.animated &&
    !props.BlockComponent &&
    !props.parseMarkdownIntoBlocksFn;
  // Streamdown keys its translations by their JSON, so an inline object is the same.
  const translationsKey = translations ? JSON.stringify(translations) : "";
  const ownIcons = Boolean(icons?.CopyIcon || icons?.DownloadIcon);
  const settings = useMemo<Settings>(
    () => ({
      plugins,
      cn: createCn(prefix),
      prefix: prefix ?? "",
      fallback: fallbackComponent !== undefined,
      highlight: highlighterFor(plugins?.code),
      whole,
      shell: ownIcons ? null : shellLabels(translationsKey),
    }),
    // biome-ignore lint/correctness/useExhaustiveDependencies: translations by value
    [plugins, prefix, fallbackComponent, whole, ownIcons, translationsKey]
  );
  useEffect(load, []);
  // Streamdown repairs streaming Markdown with remend before splitting it into blocks;
  // rill's port gives the same text, so Streamdown is told not to.
  const rill = useSyncExternalStore(subscribe, snapshot, snapshot);
  const { children } = props;
  const heals =
    typeof children === "string" &&
    props.mode !== "static" &&
    props.parseIncompleteMarkdown !== false &&
    props.remend === undefined;
  const healed = useMemo(
    () => (heals && rill?.remend ? rill.remend(children as string) : null),
    [heals, rill, children]
  );
  return createElement(
    RillContext.Provider,
    { value: settings },
    createElement(StreamdownComponent, {
      ...props,
      ...(plugins ? { plugins: streamdownPlugins(plugins, settings.highlight) } : {}),
      ...(healed === null ? {} : { children: healed, parseIncompleteMarkdown: false }),
      // The same blocks, without Streamdown's whole-text tests on every update.
      parseMarkdownIntoBlocksFn:
        props.parseMarkdownIntoBlocksFn ?? (whole ? wholeText : parseMarkdownIntoBlocks),
      BlockComponent: props.BlockComponent
        ? healed === null
          ? props.BlockComponent
          : healingBlock(props.BlockComponent)
        : Block,
    })
  );
}, sameStreamdownProps as never);
(Streamdown as { displayName?: string }).displayName = "Streamdown";

/** The text as one block when Streamdown would split it (Block splits it again). */
const wholeText = (text: string): string[] => {
  const blocks = parseMarkdownIntoBlocks(text);
  return blocks.length > 1 ? [text] : blocks;
};

const codePlugins = new WeakMap<object, PluginConfig>();

/**
 * `plugins` with rill's highlighting in its code plugin, so the blocks Streamdown's own
 * Block renders (raw HTML) highlight as rill's do: the same tokens, off the main thread.
 * Its other plugins stay the same objects (rill compares them).
 */
function streamdownPlugins(
  plugins: PluginConfig,
  highlighter: Highlighter | undefined
): PluginConfig {
  const code = plugins.code;
  if (!(code && highlighter)) return plugins;
  let wrapped = codePlugins.get(plugins);
  if (!wrapped) {
    wrapped = {
      ...plugins,
      code: {
        ...code,
        highlight: (options, callback) =>
          highlighter.highlight(options, callback ?? (() => {})),
      },
    };
    codePlugins.set(plugins, wrapped);
  }
  return wrapped;
}

const healingBlocks = new WeakMap<object, ComponentType<BlockProps>>();

/** The page's own BlockComponent, told (as Streamdown would) that remend ran. */
function healingBlock(component: ComponentType<BlockProps>): ComponentType<BlockProps> {
  let wrapped = healingBlocks.get(component);
  if (!wrapped) {
    wrapped = (props: BlockProps) =>
      createElement(component, { ...props, shouldParseIncompleteMarkdown: true });
    healingBlocks.set(component, wrapped);
  }
  return wrapped;
}

// ---------------------------------------------------------------------------------
// Which blocks rill renders
// ---------------------------------------------------------------------------------

const REMARK_DEFAULTS = Object.values(defaultRemarkPlugins);
const REHYPE_DEFAULTS = Object.values(defaultRehypePlugins);
/** Block props that reach Streamdown's Markdown and change its output. */
const MARKDOWN_OPTIONS = [
  "allowElement",
  "allowedElements",
  "disallowedElements",
  "skipHtml",
  "unwrapDisallowed",
  "urlTransform",
  "remarkRehypeOptions",
] as const;

type Pipeline = { remark: Pluggable[]; rehype: Pluggable[] };
const pipelines = new WeakMap<object, Pipeline>();
const NO_PLUGINS = {};

/** The plugin lists Streamdown merges for `plugins` with its default pipeline. */
function pipelineFor(plugins: PluginConfig | undefined): Pipeline {
  const key = plugins ?? NO_PLUGINS;
  let pipeline = pipelines.get(key);
  if (!pipeline) {
    pipeline = {
      remark: [
        ...(plugins?.cjk?.remarkPluginsBefore ?? []),
        ...REMARK_DEFAULTS,
        ...(plugins?.cjk?.remarkPluginsAfter ?? []),
        ...(plugins?.math ? [plugins.math.remarkPlugin] : []),
      ],
      rehype: [...REHYPE_DEFAULTS, ...(plugins?.math ? [plugins.math.rehypePlugin] : [])],
    };
    pipelines.set(key, pipeline);
  }
  return pipeline;
}

function sameList(left: readonly unknown[] | undefined, right: readonly unknown[]) {
  return (
    left !== undefined &&
    left.length === right.length &&
    left.every((value, index) => value === right[index])
  );
}

const defaults = defaultComponents as unknown as Record<string, unknown>;
const overridesCache = new WeakMap<object, string>();

/** The tags whose component is not Streamdown's built-in one. */
function overridesOf(components: Record<string, unknown> | undefined): string {
  if (!components) return "";
  let found = overridesCache.get(components);
  if (found === undefined) {
    found = Object.keys(components)
      .filter((tag) => components[tag] !== defaults[tag])
      .join(" ");
    overridesCache.set(components, found);
  }
  return found;
}

type Plan = { overrides: string; math: boolean; cjk: boolean };

/** How rill renders this block, or `null` to leave it to Streamdown's own Block. */
function planFor(
  props: BlockProps,
  settings: Settings,
  renderModal: boolean
): Plan | null {
  const { plugins } = settings;
  if (settings.fallback || props.animatePlugin) return null;
  const record = props as unknown as Record<string, unknown>;
  if (MARKDOWN_OPTIONS.some((name) => record[name] !== undefined)) return null;
  const pipeline = pipelineFor(plugins);
  if (
    !sameList(props.remarkPlugins as unknown[], pipeline.remark) ||
    !sameList(props.rehypePlugins as unknown[], pipeline.rehype)
  ) {
    return null;
  }
  // rill parses `$$` only (remark-math's `singleDollarTextMath: false`).
  if (
    plugins?.math &&
    optionsOf(plugins.math.remarkPlugin)?.singleDollarTextMath !== false
  ) {
    return null;
  }
  let overrides = overridesOf(props.components as Record<string, unknown>);
  // A custom modal renders next to each link button: Streamdown's link component.
  if (renderModal) overrides += " a";
  // Streamdown's components read the incomplete fence from a context only its Block
  // provides.
  if (
    props.isIncomplete &&
    (overrides.trim() !== "" || plugins?.mermaid || plugins?.renderers?.length)
  ) {
    return null;
  }
  return { overrides, math: Boolean(plugins?.math), cjk: Boolean(plugins?.cjk) };
}

const rillPipelines = new WeakMap<object, Pipeline>();

/** Streamdown's `Block` for a block rill does not render: with rill's plugins when the
 * pipeline is Streamdown's default (the same tree), else as it is. */
function fallback(props: BlockProps, settings: Settings | null, rill: Rill | null) {
  if (
    rill &&
    settings &&
    !props.animatePlugin &&
    sameList(props.remarkPlugins as unknown[], pipelineFor(settings.plugins).remark) &&
    sameList(props.rehypePlugins as unknown[], pipelineFor(settings.plugins).rehype)
  ) {
    const plugins = settings.plugins;
    const key = plugins ?? NO_PLUGINS;
    let pipeline = rillPipelines.get(key);
    if (!pipeline) {
      const own = pluginsFor(rill, plugins);
      pipeline = {
        remark: [
          ...(plugins?.cjk?.remarkPluginsBefore ?? []),
          ...own.remarkPlugins,
          ...(plugins?.cjk?.remarkPluginsAfter ?? []),
          ...(plugins?.math ? [plugins.math.remarkPlugin] : []),
        ],
        rehype: [
          ...own.rehypePlugins,
          ...(plugins?.math ? [plugins.math.rehypePlugin] : []),
        ],
      };
      rillPipelines.set(key, pipeline);
    }
    return createElement(StreamdownBlock, {
      ...props,
      remarkPlugins: pipeline.remark,
      rehypePlugins: pipeline.rehype,
    });
  }
  return createElement(StreamdownBlock, props);
}

// ---------------------------------------------------------------------------------
// Block
// ---------------------------------------------------------------------------------

/** Streamdown's Block memo comparison (index.tsx). */
function sameBlockProps(previous: BlockProps, next: BlockProps): boolean {
  if (previous.content !== next.content) return false;
  if (previous.shouldNormalizeHtmlIndentation !== next.shouldNormalizeHtmlIndentation) {
    return false;
  }
  if (previous.index !== next.index) return false;
  if (previous.isIncomplete !== next.isIncomplete) return false;
  if (previous.dir !== next.dir) return false;
  if (previous.components !== next.components) {
    const before = (previous.components ?? {}) as Record<string, unknown>;
    const after = (next.components ?? {}) as Record<string, unknown>;
    const keys = Object.keys(before);
    if (keys.length !== Object.keys(after).length) return false;
    if (keys.some((key) => before[key] !== after[key])) return false;
  }
  if (previous.rehypePlugins !== next.rehypePlugins) return false;
  if (previous.remarkPlugins !== next.remarkPlugins) return false;
  return Boolean(previous.animatePlugin) === Boolean(next.animatePlugin);
}

const noop = () => () => {};
/** `true` while rendering on the server or hydrating what it rendered. */
const useServerFormat = () => {
  const server = useSyncExternalStore(
    noop,
    () => false,
    () => true
  );
  const [pinned] = useState(server);
  return pinned;
};

/** Streamdown's `Block`, rendered by rill. Same props, same DOM. */
export const Block = memo(function Block(props: BlockProps) {
  const settings = useContext(RillContext);
  const { linkSafety, isAnimating } = useContext(StreamdownContext);
  const rill = useSyncExternalStore(subscribe, snapshot, snapshot);
  const ssr = useServerFormat();
  const [link, setLink] = useState<{ href: string; id: number } | null>(null);
  const host = useRef<HTMLElement | null>(null);
  const openLink = useRef((href: string) =>
    setLink((previous) => ({ href, id: (previous?.id ?? 0) + 1 }))
  ).current;
  const plan =
    settings && rill?.markup
      ? planFor(props, settings, Boolean(linkSafety?.renderModal))
      : null;
  const content = props.shouldNormalizeHtmlIndentation
    ? normalizeHtmlIndentation(props.content)
    : props.content;
  const safe = Boolean(linkSafety?.enabled);
  const tables = useRef<TableCache | null>(null);
  const pieces = useRef<Pieces | null>(null);
  const nodes = useMemo(() => {
    // The whole text (see `Settings.whole`): Streamdown's blocks, each rendered as a
    // Block of its own would render it.
    const blocks =
      settings?.whole && props.index === 0 ? parseMarkdownIntoBlocks(props.content) : null;
    if (settings && blocks && blocks.length > 1) {
      tables.current = null;
      pieces.current = null;
      return blocks.map((block, index) => {
        const piece = props.shouldNormalizeHtmlIndentation ? normalizeHtmlIndentation(block) : block;
        let reader: HastReader | null = null;
        if (plan && rill?.markup && piece.length <= DEFAULT_MAX_SOURCE_LENGTH) {
          try {
            reader = rill.markup(piece, plan, {
              prefix: settings.prefix,
              linkSafety: safe,
              ssr,
              overrides: plan.overrides,
              spine: false,
            });
          } catch {
            reader = null;
          }
        }
        if (!reader) {
          blockRenders.streamdown += 1;
          return createElement(
            Fragment,
            { key: index },
            fallback({ ...props, content: block, index }, settings, rill)
          );
        }
        blockRenders.rill += 1;
        const env: Env = {
          settings,
          components: props.components as Record<string, unknown>,
          isIncomplete: false,
          ssr,
          openLink,
        };
        return createElement(Fragment, { key: index }, ...segments(reader, env));
      });
    }
    if (!(plan && settings && rill?.markup) || content.length > DEFAULT_MAX_SOURCE_LENGTH) {
      tables.current = null;
      return null;
    }
    const env: Env = {
      settings,
      components: props.components as Record<string, unknown>,
      isIncomplete: props.isIncomplete,
      ssr,
      openLink,
    };
    const render = (source: string) => {
      let reader: HastReader | null = null;
      try {
        reader = rill.markup!(source, plan, {
          prefix: settings.prefix,
          linkSafety: safe,
          ssr,
          overrides: plan.overrides,
          spine: isAnimating,
        });
      } catch {
        return null;
      }
      return reader ? segments(reader, env) : null;
    };
    // A text with link definitions is one block, which a message that streams renders as
    // pieces (an update renders the last one); one that does not renders it in one call.
    const chunks =
      !settings.whole && rill.definitions && rill.splitsAt ? splitDefinitionBlock(content) : null;
    if (chunks) {
      tables.current = null;
      const markupOptions = {
        prefix: settings.prefix,
        linkSafety: safe,
        ssr,
        overrides: plan.overrides,
        spine: isAnimating,
      };
      return renderChunks(chunks, rill, plan, markupOptions, env, (pieces.current ??= newPieces()));
    }
    pieces.current = null;
    return renderGrowingTable(content, render, tables, env) ?? remember(content, render(content), tables, env);
    // `plan` is derived from these.
  }, [
    content,
    rill,
    settings,
    props.components,
    props.remarkPlugins,
    props.rehypePlugins,
    props.isIncomplete,
    props.animatePlugin,
    safe,
    linkSafety?.renderModal,
    ssr,
    isAnimating,
  ]);
  if (!nodes) {
    blockRenders.streamdown += 1;
    return fallback(props, settings, rill);
  }
  blockRenders.rill += 1;
  const activator =
    link && typeof document !== "undefined"
      ? createElement(LinkActivator, {
          key: link.id,
          href: link.href,
          host: (host.current ??= document.createElement("div")),
        })
      : null;
  return props.dir
    ? createElement(
        Fragment,
        null,
        createElement("div", { dir: props.dir, style: { display: "contents" } }, ...nodes),
        activator
      )
    : createElement(Fragment, null, ...nodes, activator);
}, sameBlockProps);
(Block as { displayName?: string }).displayName = "Block";

// ---------------------------------------------------------------------------------
// Blocks that link definitions made one: Streamdown keeps a text with `[x]: url`
// definitions in one block, so each streamed update renders all of it. rill renders it
// as the pieces Streamdown would have split it into, each with the text's definitions
// before it (a piece's links resolve as they do in the whole text), and keeps each
// piece's React nodes until its text, the definitions it could use, or its place among
// the pieces changes: an update renders the last piece.
// ---------------------------------------------------------------------------------

type Piece = {
  /** The definitions it was rendered with ("" for a piece with no `]`, which has no
   * reference). */
  definitions: string;
  last: boolean;
  /** The block's `isIncomplete` when it holds a code block (each one reads it). */
  incomplete: boolean | null;
  /** The top-level tag counts before it (its keys continue them). */
  before: string;
  nodes: ReactNode[];
  /** The top-level tags it holds, for the next piece's counts. */
  counts: Map<string, number>;
};

type Pieces = {
  rendered: Map<string, Piece>;
  /** Each piece's root-level definitions, `null` when it holds one elsewhere. */
  definitions: Map<string, string[] | null>;
  /** A piece and the next piece it was found to end before (no block spans them). */
  joins: Map<string, string>;
};

const newPieces = (): Pieces => ({
  rendered: new Map(),
  definitions: new Map(),
  joins: new Map(),
});

const countsKey = (counts: Map<string, number>) => {
  let key = "";
  for (const [tag, count] of counts) key += `${tag}:${count},`;
  return key;
};

/** `chunks` merged where micromark's blocks run across marked's (a lazy line). */
function joinChunks(chunks: string[], rill: Rill, plan: Plan, pieces: Pieces): string[] {
  const joined: string[] = [];
  const joins = new Map<string, string>();
  let current = chunks[0]!;
  for (let index = 1; index < chunks.length; index += 1) {
    const next = chunks[index]!;
    const clean =
      pieces.joins.get(current) === next ||
      rill.splitsAt!(current + next, plan, current.length);
    if (clean) {
      joins.set(current, next);
      joined.push(current);
      current = next;
    } else {
      current += next;
    }
  }
  joined.push(current);
  pieces.joins = joins;
  return joined;
}

/** Whether a piece that rendered nothing holds more than definitions (a node sanitize
 * removed, such as an HTML comment). */
function removedNode(chunk: string, definitions: string[] | null | undefined): boolean {
  let rest = chunk;
  for (const definition of definitions ?? []) rest = rest.replace(definition, "");
  return /\S/.test(rest);
}

function renderChunks(
  split: string[],
  rill: Rill,
  plan: Plan,
  options: MarkupOptions,
  env: Env,
  pieces: Pieces
): ReactNode[] | null {
  const chunks = joinChunks(split, rill, plan, pieces);
  // The text's definitions, in order (the first of a label wins, as in the whole text).
  const definitions: string[] = [];
  const known = new Map<string, string[] | null>();
  for (const chunk of chunks) {
    if (!chunk.includes("]:")) continue;
    let found = pieces.definitions.get(chunk);
    if (found === undefined) {
      const ranges = rill.definitions!(chunk, plan);
      found = ranges && ranges.map(([start, end]) => chunk.slice(start, end));
    }
    known.set(chunk, found);
    // A definition in a container: render the whole block.
    if (found === null) return null;
    definitions.push(...found);
  }
  pieces.definitions = known;
  const before = definitions.length > 0 ? `${definitions.join("\n\n")}\n\n` : "";
  const out: ReactNode[] = [];
  const counts = new Map<string, number>();
  const rendered = new Map<string, Piece>();
  /** Root children so far, and whether the last one was removed (it rendered nothing). */
  let slots = 0;
  let barrier = false;
  for (let index = 0; index < chunks.length; index += 1) {
    const chunk = chunks[index]!;
    const last = index === chunks.length - 1;
    const usable = chunk.includes("]") ? before : "";
    const at = countsKey(counts);
    let piece = pieces.rendered.get(chunk);
    if (
      !(
        piece &&
        piece.definitions === usable &&
        piece.last === last &&
        piece.before === at &&
        (piece.incomplete === null || piece.incomplete === env.isIncomplete)
      )
    ) {
      const source = usable + chunk;
      let reader: HastReader | null = null;
      try {
        reader = rill.markup!(source, plan, { ...options, spine: Boolean(options.spine) && last });
      } catch {
        return null;
      }
      if (!reader) return null;
      const own = new Map(counts);
      const coded = { code: false };
      const nodes = segments(reader, env, own, coded);
      for (const [tag, count] of counts) own.set(tag, (own.get(tag) ?? 0) - count);
      const incomplete = coded.code ? env.isIncomplete : null;
      piece = { definitions: usable, last, incomplete, before: at, nodes, counts: own };
    }
    rendered.set(chunk, piece);
    for (const [tag, count] of piece.counts) counts.set(tag, (counts.get(tag) ?? 0) + count);
    // A piece of definitions alone has no node in the whole text's root; one whose
    // nodes sanitize removed (an HTML comment) had one, with a line ending on each side.
    if (piece.nodes.length === 0 && !removedNode(chunk, known.get(chunk))) continue;
    // The root's children join with a line ending, which merges with text beside it
    // (parse5 merges adjacent text: newlines it moves out of a table), but not across a
    // removed node (it was still there when parse5 ran).
    // At the root, text is only the line endings between nodes; one that a piece starts
    // (or ends) with sits after (or before) a node of its own that sanitize removed (an
    // HTML comment, a wrapper tag's start or end), which keeps it apart.
    const opens = /^\s*</.test(chunk);
    let first = 0;
    if (slots > 0) {
      const previous = out.length - 1;
      const head = piece.nodes[0];
      if (typeof head === "string" && !opens) {
        out.push(`\n${head}`);
        first = 1;
      } else if (typeof head === "string") {
        if (!barrier && typeof out[previous] === "string") out[previous] = `${out[previous]}\n`;
        else out.push("\n");
      } else if (!barrier && typeof out[previous] === "string") {
        out[previous] = `${out[previous]}\n`;
      } else {
        out.push("\n");
      }
    }
    slots += 1;
    barrier =
      piece.nodes.length === 0 ||
      (typeof piece.nodes.at(-1) === "string" && />\s*$/.test(chunk));
    for (let node = first; node < piece.nodes.length; node += 1) out.push(piece.nodes[node]);
  }
  pieces.rendered = rendered;
  return out;
}

// ---------------------------------------------------------------------------------
// Streaming tables: a GFM table's rows parse one line at a time, so once a row's line
// has ended it renders the same however the table grows. A block that is one table
// keeps its finished rows' React elements, and rill renders only the header and the
// lines after them; React then skips the rows it already has.
// ---------------------------------------------------------------------------------

type TableCache = {
  /** The block source up to `finished`: the header and the rows the cache holds. A
   * block that still starts with it has those rows (the partial line after them may
   * change, as remend heals it). */
  source: string;
  /** The header and delimiter rows (with their line endings). */
  header: string;
  /** Where the finished lines end in `source`. */
  finished: number;
  /** The rows of those lines. */
  rows: ReactElement[];
  table: ReactElement;
  head: ReactElement;
  body: ReactElement | undefined;
};

type TableParts = {
  /** The newlines parse5 moves out of the table's structure, before it. */
  lead: string;
  table: ReactElement;
  head: ReactElement;
  body: ReactElement | undefined;
  rows: ReactElement[];
};

const childList = (element: ReactElement): ReactNode[] => {
  const children = (element.props as { children?: ReactNode }).children;
  return children === undefined ? [] : Array.isArray(children) ? children : [children];
};

/** The parts of a block rendered as exactly one table. */
function tableParts(nodes: ReactNode[] | null, env: Env): TableParts | null {
  if (!nodes || nodes.length !== 2) return null;
  const lead = nodes[0];
  if (typeof lead !== "string" || !/^\n+$/.test(lead)) return null;
  const table = nodes[1] as ReactElement;
  if (!table || table.type !== env.components.table) return null;
  const [head, body, ...rest] = childList(table) as ReactElement[];
  if (rest.length > 0 || head?.type !== "thead" || (body && body.type !== "tbody")) return null;
  const rows = body ? (childList(body) as ReactElement[]) : [];
  return rows.every((row) => row?.type === "tr") ? { lead, table, head, body, rows } : null;
}

/** A row's key at `index`: `tr-n`, with `:html` for a static one. */
const rowKey = (row: ReactElement, index: number) =>
  `tr-${index}${String(row.key).endsWith(":html") ? ":html" : ""}`;

const nonBlankLines = (text: string) =>
  text.split("\n").filter((line) => line.trim() !== "").length;

function remember(
  content: string,
  nodes: ReactNode[] | null,
  cache: { current: TableCache | null },
  env: Env
): ReactNode[] | null {
  cache.current = null;
  const parts = tableParts(nodes, env);
  if (!parts) return nodes;
  const second = content.indexOf("\n", content.indexOf("\n") + 1);
  if (second < 0) return nodes;
  const header = content.slice(0, second + 1);
  // Every line after the header is a row (the table holds the whole block).
  if (parts.rows.length !== nonBlankLines(content.slice(header.length))) return nodes;
  const finished = Math.max(header.length, content.lastIndexOf("\n") + 1);
  const done = nonBlankLines(content.slice(header.length, finished));
  cache.current = {
    source: content.slice(0, finished),
    header,
    finished,
    rows: parts.rows.slice(0, done),
    table: parts.table,
    head: parts.head,
    body: parts.body,
  };
  return nodes;
}

function renderGrowingTable(
  content: string,
  render: (source: string) => ReactNode[] | null,
  cache: { current: TableCache | null },
  env: Env
): ReactNode[] | null {
  const previous = cache.current;
  if (!(previous && content.startsWith(previous.source))) {
    return null;
  }
  const tail = content.slice(previous.finished);
  const parts = tableParts(render(previous.header + tail), env);
  const rendered = parts?.rows.length ?? 0;
  if (!parts || rendered === 0 || rendered !== nonBlankLines(tail)) return null;
  // mdast-util-to-hast's newlines around and between the c cells of each row: the
  // header's c + 5, the first body row's c + 4, each later row's c + 2, all moved before
  // the table. So the lead of `rendered` rows gives c, and each cached row adds c + 2.
  const columns = (parts.lead.length - 2 * rendered - 7) / (rendered + 1);
  if (!Number.isInteger(columns) || columns < 1) return null;
  const offset = previous.rows.length;
  const rows = previous.rows.concat(
    parts.rows.map((row, index) =>
      cloneElement(row, { key: rowKey(row, offset + index) })
    )
  );
  const body = parts.body ?? previous.body;
  const bodyProps = { ...(body?.props as object), key: body?.key } as Record<string, unknown>;
  delete bodyProps.children;
  const table = createElement(
    previous.table.type as ComponentType<Record<string, unknown>>,
    { ...(parts.table.props as object), key: parts.table.key, children: undefined },
    previous.head,
    ...(body ? [createElement("tbody", bodyProps, ...rows)] : [])
  );
  blockRenders.tableTails += 1;
  // The rows finished now join the cache.
  const finished = Math.max(previous.header.length, content.lastIndexOf("\n") + 1);
  const done = offset + nonBlankLines(content.slice(previous.finished, finished));
  cache.current = {
    ...previous,
    source: content.slice(0, finished),
    finished,
    rows: rows.slice(0, done),
    body,
  };
  return ["\n".repeat(parts.lead.length + offset * (columns + 2)), table];
}

/**
 * Opens a link as Streamdown's link component does (`onLinkCheck`, `window.open`, or
 * its safety modal): its own component, mounted outside the document, clicked once.
 */
function LinkActivator({ href, host }: { href: string; host: HTMLElement }) {
  useLayoutEffect(() => {
    host.querySelector("button")?.click();
  }, [host]);
  return createPortal(
    createElement(defaults.a as ComponentType<Record<string, unknown>>, { href }, href),
    host
  );
}

// ---------------------------------------------------------------------------------
// Segments → React
// ---------------------------------------------------------------------------------

const SEG_TEXT = 0;
const SEG_STATIC = 1;
const SEG_ELEMENT = 2;
const SEG_CODE = 3;
const SEG_TABLE = 4;
const SEG_HAST = 5;
const SEG_MATH = 6;
const SEG_LINK = 7;
const VALUE_STRING = 0;
const VALUE_TRUE = 1;
const VALUE_FALSE = 2;
const VALUE_NUMBER = 3;
const VALUE_ALIGN = 4;
const PART_HTML = 0;

type Env = {
  settings: Settings;
  components: Record<string, unknown>;
  isIncomplete: boolean;
  ssr: boolean;
  openLink: (href: string) => void;
};

const LINK_BUTTON = 'button[data-streamdown="link"]';

/** One block's segments as React nodes; `top` counts the tags of the top-level ones
 * for their keys, from the counts it holds. */
function segments(
  reader: HastReader,
  env: Env,
  top: Map<string, number> = new Map(),
  coded: { code: boolean } = { code: false }
): ReactNode[] {
  const list = (counts = new Map<string, number>()): ReactNode[] => {
    const count = reader.next();
    const out = new Array<ReactNode>(count);
    // hast-util-to-jsx-runtime's keys: `tag-n`, counting each tag among siblings.
    const key = (name: string) => {
      const count = counts.get(name) ?? 0;
      counts.set(name, count + 1);
      return `${name}-${count}`;
    };
    for (let index = 0; index < count; index += 1) {
      out[index] = segment(key);
    }
    return out;
  };
  const props = (key: string): Record<string, unknown> => {
    const out: Record<string, unknown> = { key };
    for (let count = reader.next(); count > 0; count -= 1) {
      const name = reader.string();
      const kind = reader.next();
      out[name] =
        kind === VALUE_STRING
          ? reader.string()
          : kind === VALUE_TRUE
            ? true
            : kind === VALUE_FALSE
              ? false
              : kind === VALUE_NUMBER
                ? reader.next()
                : undefined;
      if (kind === VALUE_ALIGN) {
        delete out[name];
        out.style = { textAlign: reader.string() };
      }
    }
    return out;
  };
  const segment = (keyFor: (name: string) => string): ReactNode => {
    const kind = reader.next();
    if (kind === SEG_TEXT) return reader.string();
    if (kind === SEG_HAST || kind === SEG_MATH) {
      const node = reader.node();
      const key = keyFor(node.tagName ?? "");
      if (kind === SEG_HAST) {
        const nodes = env.settings.plugins?.math ? mathOf(env)([node]) : [node];
        return createElement(Fragment, { key }, jsxOf(nodes, env.components));
      }
      const nodes = mathOf(env)([node]);
      if (env.ssr) return createElement(Fragment, { key }, jsxOf(nodes, {}));
      return createElement(
        Fragment,
        { key },
        ...nodes.map((math, index) => staticKatex(math, index, env))
      );
    }
    const key = keyFor(reader.string());
    switch (kind) {
      case SEG_STATIC: {
        const tag = reader.string();
        // Its own key: React would not clear the HTML when it turns into an element
        // with children (static parts hold no state to keep).
        const element = props(`${key}:html`);
        let html = "";
        for (let count = reader.next(); count > 0; count -= 1) {
          html += reader.next() === PART_HTML ? reader.string() : katexHtml(reader.node(), env);
        }
        const links = new Array<string>(reader.next());
        for (let index = 0; index < links.length; index += 1) {
          links[index] = reader.string();
        }
        if (links.length > 0) {
          element.onClick = (event: MouseEvent<HTMLElement>) => {
            const target = event.target as Element | null;
            const button = target?.closest?.(LINK_BUTTON);
            const scope = event.currentTarget;
            if (!(button && scope.contains(button))) return;
            const buttons = Array.from(scope.querySelectorAll(LINK_BUTTON));
            if (scope.matches(LINK_BUTTON)) buttons.unshift(scope);
            const href = links[buttons.indexOf(button)];
            if (href !== undefined) env.openLink(href);
          };
        }
        element.dangerouslySetInnerHTML = { __html: html };
        return createElement(tag, element);
      }
      case SEG_ELEMENT: {
        const tag = reader.string();
        const element = props(key);
        return createElement(tag, element, ...list());
      }
      case SEG_LINK: {
        const element = props(key);
        const href = reader.string();
        element.onClick = () => env.openLink(href);
        return createElement("button", element, ...list());
      }
      case SEG_CODE: {
        coded.code = true;
        const code = reader.string();
        const codeProps = props("");
        delete codeProps.key;
        return createElement(CodeSegment, {
          key,
          code,
          codeProps,
          isIncomplete: env.isIncomplete,
          ssr: env.ssr,
          plugins: env.settings.plugins,
          components: env.components,
          cn: env.settings.cn,
          highlight: env.settings.highlight,
          shell: env.settings.shell,
        });
      }
      case SEG_TABLE: {
        const element = props(key);
        return createElement(
          env.components.table as ComponentType<Record<string, unknown>>,
          element,
          ...list()
        );
      }
      default:
        throw new Error(`Unknown rill segment ${kind}`);
    }
  };
  return list(top);
}

function jsxOf(nodes: HastNode[], components: Record<string, unknown>): ReactElement {
  return toJsxRuntime({ type: "root", children: nodes } as never, {
    Fragment,
    components: components as never,
    ignoreInvalidStyle: true,
    jsx: jsx as never,
    jsxs: jsxs as never,
    passKeys: true,
    passNode: true,
  });
}

/** A KaTeX root as one React element (its own props from hast-util-to-jsx-runtime)
 * holding its markup as HTML. */
function staticKatex(node: HastNode, index: number, env: Env): ReactNode {
  if (node.type !== "element") {
    return node.type === "text" ? (node.value ?? "") : null;
  }
  const shell = jsxOf([{ ...node, children: [] }], {}).props as { children: ReactElement };
  return cloneElement(shell.children, {
    key: index,
    dangerouslySetInnerHTML: { __html: hastToHtml(node.children ?? [], env.ssr) },
  } as never);
}

const mathByPlugin = new WeakMap<object, ReturnType<typeof mathFragments>>();

function mathOf(env: Env) {
  const math = env.settings.plugins?.math;
  if (!math) return (nodes: HastNode[]) => nodes;
  let rewrite = mathByPlugin.get(math);
  if (!rewrite) {
    rewrite = mathFragments([math.rehypePlugin]);
    mathByPlugin.set(math, rewrite);
  }
  return rewrite as (nodes: HastNode[]) => HastNode[];
}

const katexCaches = new WeakMap<object, Map<string, string>>();

/** A math element (inside static HTML) as KaTeX's HTML. */
function katexHtml(node: HastNode, env: Env): string {
  const math = env.settings.plugins?.math ?? NO_PLUGINS;
  let katexCache = katexCaches.get(math);
  if (!katexCache) {
    katexCache = new Map();
    katexCaches.set(math, katexCache);
  }
  const key = `${env.ssr ? 1 : 0}${node.tagName}\0${text(node)}`;
  let html = katexCache.get(key);
  if (html === undefined) {
    html = hastToHtml(mathOf(env)([node]), env.ssr);
    if (katexCache.size > 1024) katexCache.clear();
    katexCache.set(key, html);
  }
  return html;
}

function text(node: HastNode): string {
  return node.type === "text"
    ? (node.value ?? "")
    : (node.children ?? []).map(text).join("");
}
