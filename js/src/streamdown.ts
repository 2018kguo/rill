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
import type { PluggableList } from "unified";
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
  type StreamdownProps as BaseStreamdownProps,
} from "streamdown";
import type { Pluggable } from "unified";

import { parseMarkdownIntoBlocks, splitDefinitionBlock } from "./blocks.ts";
import { CodeSegment, shellLabels, type ShellLabels } from "./code-block.ts";
import {
  argsKey,
  HastReader,
  type Args,
  type HastNode,
  type MarkupOptions,
  type Rill,
  type Syntax,
} from "./core.ts";
import type { Engine, Pipeline } from "./engine.ts";
import { rillEngine } from "./index.ts";
import { highlighterFor, type Highlighter } from "./highlight.ts";
import { createCn, hastToHtml, type Cn } from "./html.ts";
import { mathFragments } from "./math.ts";
import { DEFAULT_MAX_SOURCE_LENGTH, optionsOf } from "./plugins.ts";
import { load, loadFailed, pluginsFor } from "./react.ts";
import { reportFallback, type FallbackReason } from "./fallback.ts";
import { knownLists } from "./known.ts";
import {
  animatedKey,
  beginPass,
  blockAnimation,
  commitBlock,
  endPass,
  newAnimation,
  streamdownPlugin,
  type Animation,
  type BlockAnimation,
} from "./animate.ts";

// Everything else Streamdown 2.7.1 exports, by name: Next.js rejects `export *` in a
// "use client" module that a server component imports (test/exports.test.ts keeps the
// list complete).
export {
  CodeBlock,
  CodeBlockContainer,
  CodeBlockCopyButton,
  CodeBlockDownloadButton,
  CodeBlockHeader,
  CodeBlockSkeleton,
  createAnimatePlugin,
  defaultComponents,
  defaultRehypePlugins,
  defaultRemarkPlugins,
  defaultTranslations,
  defaultUrlTransform,
  detectTextDirection,
  escapeMarkdownTableCell,
  extractTableDataFromElement,
  normalizeHtmlIndentation,
  StreamdownContext,
  TableCopyDropdown,
  TableDownloadButton,
  TableDownloadDropdown,
  tableDataToCSV,
  tableDataToMarkdown,
  tableDataToTSV,
  useIsCodeFenceIncomplete,
  type AllowElement,
  type AllowedTags,
  type AnimateOptions,
  type BlockProps,
  type BundledLanguage,
  type BundledTheme,
  type CSVSeparator,
  type CjkPlugin,
  type CodeHighlighterPlugin,
  type Components,
  type ControlsConfig,
  type CopyControlConfig,
  type CustomRenderer,
  type CustomRendererProps,
  type DefaultComponents,
  type DiagramPlugin,
  type DownloadControlConfig,
  type ExtraProps,
  type HighlightOptions,
  type HighlightResult,
  type HighlightToken,
  type IconMap,
  type LinkSafetyConfig,
  type LinkSafetyModalProps,
  type MathPlugin,
  type MermaidErrorComponentProps,
  type MermaidOptions,
  type PluginConfig,
  type PortalTarget,
  type StreamdownContextType,
  type StreamdownTranslations,
  type TableCopyDropdownProps,
  type TableData,
  type TableDownloadButtonProps,
  type TableDownloadDropdownProps,
  type ThemeInput,
  type ThemeRegistrationAny,
  type UrlTransform,
} from "streamdown";
export { defineEngine, type Engine, type EngineDefinition, type Pipeline } from "./engine.ts";
export { rillEngine } from "./index.ts";
export {
  DEFAULT_MAX_SOURCE_LENGTH,
  rillPlugins,
  type RenderKind,
  type RillPluginsOptions,
} from "./plugins.ts";
export { useRill } from "./react.ts";
export {
  setFallbackListener,
  type FallbackListener,
  type FallbackReason,
} from "./fallback.ts";
// Streamdown's block splitter, without its whole-text tests on every streaming update.
export { parseMarkdownIntoBlocks } from "./blocks.ts";
export {
  highlightCounts,
  pendingHighlights,
  preloadHighlighting,
  setHighlightWasm,
  setRillHighlight,
} from "./highlight.ts";

/**
 * Streamdown 2.7.1's props, plus the engine that renders them: `rillEngine` (Streamdown's
 * own pipeline) by default, or a custom build (`defineEngine`) with `engineArgs`, its
 * extra arguments, compared by value. An engine brings its remark and rehype plugins,
 * so `remarkPlugins` and `rehypePlugins` go with the default engine only.
 */
export type StreamdownProps<A extends Args = []> =
  | (BaseStreamdownProps & { engine?: undefined; engineArgs?: undefined })
  | (Omit<BaseStreamdownProps, "remarkPlugins" | "rehypePlugins"> & {
      engine: Engine<A>;
      remarkPlugins?: never;
      rehypePlugins?: never;
    } & ([] extends A ? { engineArgs?: A } : { engineArgs: A }));

/** What `Block` needs from `<Streamdown>`'s props that its own props lack. */
type Settings = {
  /** rill for the engine and its arguments, once loaded. */
  rill: Rill | null;
  engine: Engine<Args>;
  /** Why the blocks' plugin lists are not the engine's, when the props say. */
  lists: Why | null;
  /** The plugins of the page's own lists the prebuilt build renders itself. */
  syntax: Syntax | null;
  /** The remark and rehype plugins Streamdown merges `plugins` around (the engine's). */
  expected: Merged;
  /** The engine's plugins are Streamdown's defaults: the blocks rill does not render as
   * markup render with its plugins tier (`rillPlugins`). */
  defaults: boolean;
  /** The rehype plugin that renders math rill leaves to KaTeX (the engine's, or
   * `plugins.math`'s), if any. */
  math: Pluggable | undefined;
  /** `mode="static"`: the whole text renders as one block, as Streamdown's static mode
   * renders it; `isAnimating` as given. */
  static: { isAnimating: boolean } | null;
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
  /** `animated`: its timeline and the blocks' state (rill runs it while streaming). */
  animation: Animation | null;
};

const RillContext = createContext<Settings | null>(null);


/** How many block renders rill took and how many went to Streamdown's own Block
 * (diagnostics, for tests and benchmarks). */
export const blockRenders = { rill: 0, streamdown: 0, tableTails: 0 };

type Props = BaseStreamdownProps & {
  engine?: Engine<Args> | undefined;
  engineArgs?: Args | undefined;
} & Record<string, unknown>;

/** Streamdown's memo comparison (index.tsx), so this re-renders exactly when it does,
 * and for another engine or other engine arguments (by value). */
function sameStreamdownProps(previous: Props, next: Props): boolean {
  return (
    previous.engine === next.engine &&
    argsKey(previous.engineArgs ?? NO_ARGS) === argsKey(next.engineArgs ?? NO_ARGS) &&
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

const NO_ARGS: Args = [];

/** Why a block renders with Streamdown's own Block. */
type Why = { reason: FallbackReason; detail?: string };

const nameOf = (plugin: unknown): string => {
  const attacher = Array.isArray(plugin) ? plugin[0] : plugin;
  return (typeof attacher === "function" && attacher.name) || "anonymous";
};

/** The plugins of `given` that `known` lacks, by name. */
function unknownPlugins(given: readonly unknown[], known: readonly unknown[]): string {
  return given
    .filter((plugin) => !known.includes(plugin))
    .map(nameOf)
    .join(", ");
}

const nonEmpty = (value: unknown) =>
  value !== undefined &&
  value !== null &&
  (Array.isArray(value) ? value.length > 0 : Object.keys(value as object).length > 0);

/** Why Streamdown hands the blocks plugin lists other than the engine's, if its props
 * say why. */
function listsWhy(props: Props, pipeline: Pipeline): Why | null {
  const remark = props.remarkPlugins as unknown[] | undefined;
  const rehype = props.rehypePlugins as unknown[] | undefined;
  if (props.engine && (remark !== undefined || rehype !== undefined)) {
    return { reason: "engine-lists" };
  }
  if (remark && !sameList(remark, pipeline.remarkPlugins as unknown[])) {
    return {
      reason: "remarkPlugins",
      detail: unknownPlugins(remark, pipeline.remarkPlugins as unknown[]),
    };
  }
  if (rehype && !sameList(rehype, pipeline.rehypePlugins as unknown[])) {
    return {
      reason: "rehypePlugins",
      detail: unknownPlugins(rehype, pipeline.rehypePlugins as unknown[]),
    };
  }
  for (const name of ["allowedTags", "literalTagContent", "disableAutolinkProtocols"] as const) {
    if (nonEmpty(props[name])) return { reason: name };
  }
  return null;
}

/** Why a render that threw left its block to Streamdown. */
function thrown(error: unknown): Why {
  return error instanceof WebAssembly.RuntimeError ||
    error instanceof RangeError ||
    (error instanceof Error && error.message.includes("trapped"))
    ? { reason: "trapped" }
    : { reason: "error", detail: error instanceof Error ? error.message : String(error) };
}

// The prebuilt WebAssembly starts loading when the page loads this module, not when the
// first <Streamdown> mounts: a chat that fetches its messages would otherwise render
// them all with Streamdown's Block while it loads, and then again with rill. One task
// later, so a page that calls `initRill` from its own module (bytes of its own) wins.
if (typeof window !== "undefined") setTimeout(() => load(rillEngine), 0);

/** The props that give Streamdown the engine's plugins: none when they are its own
 * defaults (Streamdown then keeps its own lists, which some of its props look for). */
function listProps(pipeline: Pipeline): Partial<BaseStreamdownProps> {
  return sameList(pipeline.remarkPlugins, REMARK_DEFAULTS) &&
    sameList(pipeline.rehypePlugins, REHYPE_DEFAULTS)
    ? {}
    : { remarkPlugins: pipeline.remarkPlugins, rehypePlugins: pipeline.rehypePlugins };
}

const givenPipelines = new WeakMap<object, WeakMap<object, Pipeline>>();

/** A page's own lists as a pipeline: the same object for the same lists. */
function givenPipeline(remark: PluggableList, rehype: PluggableList): Pipeline {
  let byRehype = givenPipelines.get(remark);
  if (!byRehype) {
    byRehype = new WeakMap();
    givenPipelines.set(remark, byRehype);
  }
  let found = byRehype.get(rehype);
  if (!found) {
    found = { remarkPlugins: remark, rehypePlugins: rehype };
    byRehype.set(rehype, found);
  }
  return found;
}

/** A static text as one block, which `Block` renders whole. */
const staticBlocks = (text: string): string[] => [text];

/** Streamdown 2.7.1's `<Streamdown>`, rendering its blocks with rill. */
export const Streamdown = memo(function Streamdown(givenProps: StreamdownProps<Args>) {
  const props = givenProps as Props;
  const { plugins, prefix, fallbackComponent, translations, icons } = props;
  const engine = props.engine ?? (rillEngine as Engine<Args>);
  const args = props.engineArgs ?? NO_ARGS;
  const pipeline = engine.plugins(...args);
  const engineMath = engine.math(...args) as Pluggable | undefined;
  const rill = useSyncExternalStore(
    engine.subscribe,
    () => engine.rill(...args),
    () => engine.rill(...args)
  );
  useEffect(() => load(engine), [engine]);
  const ownLists = props.remarkPlugins !== undefined || props.rehypePlugins !== undefined;
  // Streamdown's static mode renders the whole text in one pass: the same as one block
  // (no remend, no caret), which rill renders whole. A `dir` sits on the root there.
  const staticMode = props.mode === "static" && props.dir === undefined;
  if (props.mode === "static" && !staticMode) reportFallback("static-dir");
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
  const isAnimating = Boolean(props.isAnimating);
  // `animated` (Streamdown's word animation while streaming): rill keeps its timeline,
  // so that its blocks and Streamdown's share it. With a BlockComponent of the page's
  // own, Streamdown keeps it (the blocks get its plugin, and render through Streamdown).
  const animationKey = props.mode === "static" || props.BlockComponent ? "" : animatedKey(props.animated);
  const animation = useRef<Animation | null>(null);
  if (!animationKey) animation.current = null;
  else if (animation.current?.key !== animationKey) {
    animation.current = newAnimation(animationKey, props.animated!);
  }
  if (animation.current && isAnimating) {
    animation.current.timeline.beginPass(animation.current.timeline.now());
  }
  useLayoutEffect(() => {
    if (isAnimating) animation.current?.timeline.commitPass();
  });
  // A page's own lists that the prebuilt build knows (see known.ts) render through rill
  // as Streamdown's do.
  const known =
    !props.engine && rill && ownLists
      ? knownLists(
          rill,
          (props.remarkPlugins ?? REMARK_DEFAULTS) as unknown[],
          (props.rehypePlugins ?? REHYPE_DEFAULTS) as unknown[],
          Boolean(plugins?.math)
        )
      : null;
  const recognized = known && "syntax" in known ? known : null;
  const blockLists = recognized
    ? givenPipeline(props.remarkPlugins ?? REMARK_DEFAULTS, props.rehypePlugins ?? REHYPE_DEFAULTS)
    : pipeline;
  const settings = useMemo<Settings>(
    () => ({
      rill,
      engine,
      lists: known && !recognized ? (known as Why) : listsWhy(props, pipeline),
      syntax: recognized?.syntax ?? null,
      expected: merged(plugins, blockLists),
      defaults: listProps(pipeline).remarkPlugins === undefined && !ownLists,
      math: recognized?.katex
        ? plugins?.math
          ? MATH_TWICE
          : recognized.katex
        : engineMath && plugins?.math
          ? MATH_TWICE
          : (engineMath ?? plugins?.math?.rehypePlugin),
      static: staticMode ? { isAnimating } : null,
      plugins,
      cn: createCn(prefix),
      prefix: prefix ?? "",
      fallback: fallbackComponent !== undefined,
      highlight: highlighterFor(plugins?.code),
      whole,
      shell: ownIcons ? null : shellLabels(translationsKey),
      animation: animation.current,
    }),
    // biome-ignore lint/correctness/useExhaustiveDependencies: translations by value
    [rill, engine, known, blockLists, animation.current, pipeline, props.remarkPlugins, props.rehypePlugins, props.allowedTags, props.literalTagContent, props.disableAutolinkProtocols, engineMath, staticMode, isAnimating, plugins, prefix, fallbackComponent, whole, ownIcons, translationsKey]
  );
  // Streamdown repairs streaming Markdown with remend before splitting it into blocks;
  // rill's port gives the same text, so Streamdown is told not to.
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
  const {
    engine: _engine,
    engineArgs: _engineArgs,
    ...streamdownProps
  } = props;
  if (staticMode) {
    // Streamdown's streaming path with one block, no remend and no caret renders what its
    // static path does; `Block` gives the components the context static mode would.
    return createElement(
      RillContext.Provider,
      { value: settings },
      createElement(StreamdownComponent, {
        ...streamdownProps,
        ...(ownLists ? {} : listProps(pipeline)),
        ...(plugins ? { plugins: streamdownPlugins(plugins, settings.highlight) } : {}),
        mode: "streaming",
        isAnimating: false,
        caret: undefined,
        animated: undefined,
        onAnimationStart: undefined,
        onAnimationEnd: undefined,
        parseIncompleteMarkdown: false,
        normalizeHtmlIndentation: false,
        parseMarkdownIntoBlocksFn: staticBlocks,
        BlockComponent: Block,
      } as unknown as BaseStreamdownProps)
    );
  }
  return createElement(
    RillContext.Provider,
    { value: settings },
    createElement(StreamdownComponent, {
      ...streamdownProps,
      ...(ownLists ? {} : listProps(pipeline)),
      ...(plugins ? { plugins: streamdownPlugins(plugins, settings.highlight) } : {}),
      ...(healed === null ? {} : { children: healed, parseIncompleteMarkdown: false }),
      ...(animation.current ? { animated: undefined } : {}),
      // The same blocks, without Streamdown's whole-text tests on every update.
      parseMarkdownIntoBlocksFn:
        props.parseMarkdownIntoBlocksFn ?? (whole ? wholeText : parseMarkdownIntoBlocks),
      BlockComponent: props.BlockComponent
        ? healed === null
          ? props.BlockComponent
          : healingBlock(props.BlockComponent)
        : Block,
    } as BaseStreamdownProps)
  );
}, sameStreamdownProps as never) as unknown as (<A extends Args = []>(
  props: StreamdownProps<A>
) => ReactElement) & { displayName?: string };
Streamdown.displayName = "Streamdown";

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

type Merged = { remark: Pluggable[]; rehype: Pluggable[] };
const mergedByPipeline = new WeakMap<Pipeline, WeakMap<object, Merged>>();
const NO_PLUGINS = {};
/** A build with math of its own and `plugins.math` too: KaTeX would run twice. */
const MATH_TWICE: Pluggable = [() => undefined];

/** The plugin lists Streamdown merges for `plugins` around an engine's plugins
 * (index.tsx `mergedRemarkPlugins`, `mergedRehypePlugins`). */
function merged(plugins: PluginConfig | undefined, pipeline: Pipeline): Merged {
  const key = plugins ?? NO_PLUGINS;
  let byPlugins = mergedByPipeline.get(pipeline);
  if (!byPlugins) {
    byPlugins = new WeakMap();
    mergedByPipeline.set(pipeline, byPlugins);
  }
  let lists = byPlugins.get(key);
  if (!lists) {
    lists = {
      remark: [
        ...(plugins?.cjk?.remarkPluginsBefore ?? []),
        ...(pipeline.remarkPlugins as Pluggable[]),
        ...(plugins?.cjk?.remarkPluginsAfter ?? []),
        ...(plugins?.math ? [plugins.math.remarkPlugin] : []),
      ],
      rehype: [
        ...(pipeline.rehypePlugins as Pluggable[]),
        ...(plugins?.math ? [plugins.math.rehypePlugin] : []),
      ],
    };
    byPlugins.set(key, lists);
  }
  return lists;
}

function sameList(left: PluggableList | readonly unknown[] | undefined, right: readonly unknown[]) {
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

type Plan = Syntax & {
  overrides: string;
  math: boolean;
  cjk: boolean;
  katex: boolean;
  incomplete: boolean;
};

/** How rill renders this block, or why it leaves it to Streamdown's own Block. */
function planFor(
  props: BlockProps,
  settings: Settings,
  renderModal: boolean
): Plan | Why {
  const { plugins } = settings;
  if (settings.fallback) return { reason: "fallbackComponent" };
  if (props.animatePlugin) return { reason: "animated-block-component" };
  const record = props as unknown as Record<string, unknown>;
  const option = MARKDOWN_OPTIONS.find((name) => record[name] !== undefined);
  if (option) return { reason: option };
  if (
    !sameList(props.remarkPlugins as unknown[], settings.expected.remark) ||
    !sameList(props.rehypePlugins as unknown[], settings.expected.rehype)
  ) {
    return settings.lists ?? { reason: "rehypePlugins" };
  }
  if (settings.math === MATH_TWICE) return { reason: "math-twice" };
  let overrides = overridesOf(props.components as Record<string, unknown>);
  // A custom modal renders next to each link button: Streamdown's link component.
  if (renderModal) overrides += " a";
  // Streamdown's components read the incomplete fence from a context only its Block
  // provides: its diagram renderers always, a page's own components where the block
  // holds one (Rust checks, see `incomplete`).
  if (props.isIncomplete && (plugins?.mermaid || plugins?.renderers?.length)) {
    return { reason: "incomplete-diagram" };
  }
  return {
    ...settings.syntax,
    overrides,
    math: Boolean(plugins?.math) || settings.syntax?.math === true,
    cjk: Boolean(plugins?.cjk),
    // `plugins.math` with remark-math's own default, `$x$` math too.
    ...(plugins?.math && optionsOf(plugins.math.remarkPlugin)?.singleDollarTextMath !== false
      ? { singleDollar: true }
      : {}),
    katex: settings.math !== undefined,
    incomplete: props.isIncomplete && overrides.trim() !== "",
  };
}

const isWhy = (plan: Plan | Why | null): plan is Why => plan !== null && "reason" in plan;

/** Why rill is not there to render (`Settings.rill` is `null`). */
function missing(settings: Settings | null): Why {
  if (typeof window === "undefined") return { reason: "server" };
  return { reason: settings && loadFailed(settings.engine) ? "load-failed" : "loading" };
}

const rillPipelines = new WeakMap<Rill, WeakMap<object, Merged>>();

/** Streamdown's `Block` for a block rill does not render: with rill's plugins when the
 * pipeline is Streamdown's default (the same tree), else as it is. */
function fallback(props: BlockProps, settings: Settings | null, animating?: BlockAnimation) {
  if (animating && settings?.animation) {
    // Streamdown's plugin on rill's timeline, with what rill's passes showed.
    const own = fallback({ ...props }, settings) as ReactElement<BlockProps>;
    const { plugin, rehypePlugins } = streamdownPlugin(
      settings.animation,
      animating,
      own.props.rehypePlugins as Pluggable[]
    );
    return createElement(StreamdownBlock, { ...own.props, animatePlugin: plugin, rehypePlugins });
  }
  const rill = settings?.rill;
  if (
    rill &&
    settings.defaults &&
    !props.animatePlugin &&
    sameList(props.remarkPlugins as unknown[], settings.expected.remark) &&
    sameList(props.rehypePlugins as unknown[], settings.expected.rehype)
  ) {
    const plugins = settings.plugins;
    const key = plugins ?? NO_PLUGINS;
    let byPlugins = rillPipelines.get(rill);
    if (!byPlugins) {
      byPlugins = new WeakMap();
      rillPipelines.set(rill, byPlugins);
    }
    let pipeline = byPlugins.get(key);
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
      byPlugins.set(key, pipeline);
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
  const context = useContext(StreamdownContext);
  const { linkSafety } = context;
  const isAnimating = context.isAnimating && !settings?.static;
  // `animated` while streaming: this block's state on the message's timeline.
  const animating =
    settings?.animation && isAnimating ? blockAnimation(settings.animation, props.index) : undefined;
  // A block that was animated settles as Streamdown's does: React drops its animation
  // from the same elements.
  const animated = settings?.animation?.blocks[props.index];
  const settled = !animating && animated !== undefined;
  const rill = settings?.rill ?? null;
  const ssr = useServerFormat();
  // Static mode: the context Streamdown's static path gives its components.
  const staticContext = settings?.static;
  const ownContext = useMemo(
    () =>
      staticContext
        ? { ...context, mode: "static" as const, isAnimating: staticContext.isAnimating }
        : null,
    [context, staticContext]
  );
  const [link, setLink] = useState<{ href: string; id: number } | null>(null);
  const host = useRef<HTMLElement | null>(null);
  const openLink = useRef((href: string) =>
    setLink((previous) => ({ href, id: (previous?.id ?? 0) + 1 }))
  ).current;
  const planned =
    settings && rill?.markup
      ? planFor(props, settings, Boolean(linkSafety?.renderModal))
      : null;
  const plan = isWhy(planned) ? null : planned;
  const content = props.shouldNormalizeHtmlIndentation
    ? normalizeHtmlIndentation(props.content)
    : props.content;
  const safe = Boolean(linkSafety?.enabled);
  const tables = useRef<TableCache | null>(null);
  const pieces = useRef<Pieces | null>(null);
  /** Why the last render left the block to Streamdown. */
  const why = useRef<Why | null>(null);
  const nodes = useMemo(() => {
    why.current = null;
    /** One block's reader, or why Streamdown renders it. */
    const read = (source: string, options: MarkupOptions): HastReader | Why => {
      if (source.length > DEFAULT_MAX_SOURCE_LENGTH) return { reason: "too-long" };
      try {
        const reader = rill!.markup!(source, plan!, options);
        return typeof reader === "string" ? { reason: reader as FallbackReason } : reader;
      } catch (error) {
        return thrown(error);
      }
    };
    // `animated`: the animate plugin's pass on the whole block, in Rust.
    if ((animating || settled) && settings?.animation) {
      tables.current = null;
      pieces.current = null;
      if (!(plan && rill?.markup)) {
        why.current = (planned as Why | null) ?? missing(settings);
        return null;
      }
      // Math (which the plugin animates after KaTeX) renders through Streamdown's Block,
      // in every block of the message: a block that turned to it mid-stream would
      // remount the elements Streamdown keeps.
      if (plan.katex || animated?.streamdown) {
        why.current = { reason: "animated-math" };
        return null;
      }
      if (animating && !settings.animation.plain) {
        why.current = { reason: "animated-options" };
        return null;
      }
      const pass = animating ? beginPass(settings.animation, animating) : undefined;
      const reader = read(content, {
        prefix: settings.prefix,
        linkSafety: safe,
        ssr,
        overrides: plan.overrides,
        katex: plan.katex,
        incomplete: plan.incomplete,
        deep: true,
        ...(pass ? { animate: pass } : {}),
      });
      if (!(reader instanceof HastReader)) {
        why.current = reader;
        return null;
      }
      if (animating && pass) endPass(settings.animation, animating, pass);
      return segments(reader, {
        settings,
        components: props.components as Record<string, unknown>,
        isIncomplete: props.isIncomplete,
        ssr,
        openLink,
      });
    }
    // The whole text (see `Settings.whole`): Streamdown's blocks, each rendered as a
    // Block of its own would render it.
    const blocks =
      settings?.whole && props.index === 0 ? parseMarkdownIntoBlocks(props.content) : null;
    if (settings && blocks && blocks.length > 1) {
      tables.current = null;
      pieces.current = null;
      return blocks.map((block, index) => {
        const piece = props.shouldNormalizeHtmlIndentation ? normalizeHtmlIndentation(block) : block;
        const reader: HastReader | Why =
          plan && rill?.markup
            ? read(piece, {
                prefix: settings.prefix,
                linkSafety: safe,
                ssr,
                overrides: plan.overrides,
                spine: false,
                katex: plan.katex,
                incomplete: plan.incomplete,
              })
            : (planned as Why | null) ?? missing(settings);
        if (!(reader instanceof HastReader)) {
          blockRenders.streamdown += 1;
          reportFallback(reader.reason, reader.detail);
          return createElement(
            Fragment,
            { key: index },
            fallback({ ...props, content: block, index }, settings)
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
    if (!(plan && settings && rill?.markup)) {
      tables.current = null;
      why.current = (planned as Why | null) ?? missing(settings);
      return null;
    }
    const env: Env = {
      settings,
      components: props.components as Record<string, unknown>,
      isIncomplete: props.isIncomplete,
      ssr,
      openLink,
    };
    const options: MarkupOptions = {
      prefix: settings.prefix,
      linkSafety: safe,
      ssr,
      overrides: plan.overrides,
      spine: isAnimating,
      katex: plan.katex,
      incomplete: plan.incomplete,
    };
    const render = (source: string) => {
      const reader = read(source, options);
      if (reader instanceof HastReader) return segments(reader, env);
      why.current = reader;
      return null;
    };
    // Static mode renders the whole text in one call.
    if (settings.static) {
      tables.current = null;
      pieces.current = null;
      return render(content);
    }
    // A text with link definitions is one block, which a message that streams renders as
    // pieces (an update renders the last one); one that does not renders it in one call.
    // rehype-slug numbers repeated headings across the whole block.
    const chunks =
      !settings.whole && !plan.slug && rill.definitions && rill.splitsAt && rill.piece && content.length <= DEFAULT_MAX_SOURCE_LENGTH
        ? splitDefinitionBlock(content)
        : null;
    if (chunks) {
      tables.current = null;
      const rendered = renderChunks(chunks, rill, plan, options, env, (pieces.current ??= newPieces()));
      if (rendered) return rendered;
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
    animating,
    settled,
  ]);
  // The plugin's commit, once the render is on screen: what it showed is shown.
  const viaStreamdown = !nodes;
  useLayoutEffect(() => {
    if (animating) {
      commitBlock(animating, viaStreamdown);
      if (viaStreamdown) animating.streamdown = true;
    }
  });
  if (!nodes) {
    blockRenders.streamdown += 1;
    const reason = why.current ?? missing(settings);
    reportFallback(reason.reason, reason.detail);
    const block = fallback(props, settings, animating);
    return ownContext ? createElement(StreamdownContext.Provider, { value: ownContext }, block) : block;
  }
  blockRenders.rill += 1;
  const activator =
    link && typeof document !== "undefined"
      ? createElement(LinkActivator, {
          // Not a number: the blocks beside it are keyed by their index.
          key: `link:${link.id}`,
          href: link.href,
          host: (host.current ??= document.createElement("div")),
        })
      : null;
  const out = props.dir
    ? createElement(
        Fragment,
        null,
        createElement("div", { dir: props.dir, style: { display: "contents" } }, ...nodes),
        activator
      )
    : createElement(Fragment, null, ...nodes, activator);
  return ownContext ? createElement(StreamdownContext.Provider, { value: ownContext }, out) : out;
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
  /** Its text. */
  chunk: string;
  /** The definitions it was rendered with: those whose label its text may hold ("" for
   * a piece with no `]`, which has no reference). */
  definitions: string;
  /** The defined footnote labels and the calls before it, when it may call one (or is
   * the footnote section); "" otherwise. */
  footnotes: string;
  /** The footnote calls after it (as `Rill.piece` returns them). */
  calls: string;
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
  /** The pieces rendered last time, in order. */
  rendered: Piece[];
  /** Each piece's root-level definitions, `null` when it holds one elsewhere. */
  definitions: Map<string, string[] | null>;
  /** Each piece's root-level footnote definitions with their labels, `null` when it
   * holds one elsewhere. */
  notes: Map<string, [string, string][] | null>;
  /** Per place between two pieces, the pieces found to split there (no block spans
   * them). */
  joins: [string, string][];
  references: References;
};

const newPieces = (): Pieces => ({
  rendered: [],
  definitions: new Map(),
  notes: new Map(),
  joins: [],
  references: newReferences(),
});

// A piece renders with the definitions whose label its text may reference, so that a
// definition arriving late renders again only the pieces that may use it. A reference
// matches a definition by its label, compared as micromark's `normalizeIdentifier`
// does (whitespace collapsed, case folded by `toLowerCase().toUpperCase()`), on the
// label's source text. So a piece may reference a label only when each of the label's
// words is in its text, normalized the same way: a word never spans a line (where a
// container's markers sit), and with backslashes dropped on both sides (a table cell's
// `\|` is a `|` in its label) the test only grows more permissive.

/** A piece's text as the label test reads it. */
type Referrer = {
  text: string;
  /** The labels it may reference. */
  labels: string[];
  /** The update it was last part of. */
  seen: number;
};

type References = {
  /** Each definition's label words (normalized), keyed by the definition's text. */
  labels: Map<string, string>;
  referrers: Map<string, Referrer>;
  /** The referrers each label's words are all found in. */
  byLabel: Map<string, Set<Referrer>>;
  update: number;
};

const newReferences = (): References => ({
  labels: new Map(),
  referrers: new Map(),
  byLabel: new Map(),
  update: 0,
});

const normalizeForLabels = (text: string) =>
  text.replace(/\\/g, "").replace(/[\t\n\r ]+/g, " ").toLowerCase().toUpperCase();

/** A definition's label, normalized (see above). */
function labelOf(definition: string): string {
  const start = definition.indexOf("[") + 1;
  let end = start;
  while (end < definition.length && definition[end] !== "]") {
    end += definition[end] === "\\" ? 2 : 1;
  }
  return normalizeForLabels(definition.slice(start, end)).trim();
}

const mayReference = (referrer: Referrer, label: string) =>
  label.split(" ").every((word) => referrer.text.includes(word));

/** For each piece text with a `]`, the definitions (in order) whose label it may
 * reference, joined as they go before it. */
function definitionsFor(
  references: References,
  chunks: string[],
  definitions: string[]
): Map<string, string> {
  const update = (references.update += 1);
  const labels = definitions.map((definition) => {
    let label = references.labels.get(definition);
    if (label === undefined) {
      label = labelOf(definition);
      references.labels.set(definition, label);
    }
    return label;
  });
  const current: Referrer[] = [];
  for (const chunk of chunks) {
    if (!chunk.includes("]")) continue;
    let referrer = references.referrers.get(chunk);
    if (!referrer) {
      referrer = { text: normalizeForLabels(chunk), labels: [], seen: update };
      references.referrers.set(chunk, referrer);
      for (const [label, found] of references.byLabel) {
        if (mayReference(referrer, label)) {
          found.add(referrer);
          referrer.labels.push(label);
        }
      }
    }
    referrer.seen = update;
    current.push(referrer);
  }
  for (const label of labels) {
    if (references.byLabel.has(label)) continue;
    const found = new Set<Referrer>();
    for (const referrer of current) {
      if (mayReference(referrer, label)) {
        found.add(referrer);
        referrer.labels.push(label);
      }
    }
    references.byLabel.set(label, found);
  }
  // Pieces that are gone (the last piece's text changes as it grows).
  for (const [chunk, referrer] of references.referrers) {
    if (referrer.seen === update) continue;
    references.referrers.delete(chunk);
    for (const label of referrer.labels) references.byLabel.get(label)?.delete(referrer);
  }
  if (references.labels.size > 4 * definitions.length + 64) {
    references.labels = new Map(definitions.map((definition, index) => [definition, labels[index]!]));
  }
  const used = new Map<Referrer, string>();
  for (const [index, definition] of definitions.entries()) {
    for (const referrer of references.byLabel.get(labels[index]!) ?? []) {
      used.set(referrer, `${used.get(referrer) ?? ""}${definition}\n\n`);
    }
  }
  const out = new Map<string, string>();
  for (const chunk of chunks) {
    const referrer = references.referrers.get(chunk);
    const usable = referrer && used.get(referrer);
    if (usable) out.set(chunk, usable);
  }
  return out;
}

const countsKey = (counts: Map<string, number>) => {
  let key = "";
  for (const [tag, count] of counts) key += `${tag}:${count},`;
  return key;
};

/** `chunks` merged where micromark's blocks run across marked's (a lazy line). */
function joinChunks(chunks: string[], rill: Rill, plan: Plan, pieces: Pieces): string[] {
  const joined: string[] = [];
  const joins: [string, string][] = [];
  let current = chunks[0]!;
  for (let index = 1; index < chunks.length; index += 1) {
    const next = chunks[index]!;
    const known = pieces.joins[index - 1];
    const clean =
      (known !== undefined && known[0] === current && known[1] === next) ||
      rill.splitsAt!(current + next, plan, current.length);
    if (clean) {
      joins[index - 1] = [current, next];
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
  // The text's definitions, in order (the first of a label wins, as in the whole text),
  // and its footnote definitions, with their labels.
  const definitions: string[] = [];
  const known = new Map<string, string[] | null>();
  const notes: string[] = [];
  const labels: string[] = [];
  const knownNotes = new Map<string, [string, string][] | null>();
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
    if (!chunk.includes("[^")) continue;
    let foundNotes = knownNotes.get(chunk) ?? pieces.notes.get(chunk);
    if (foundNotes === undefined) {
      const ranges = rill.footnotes!(chunk, plan);
      foundNotes = ranges && ranges.map(([start, end, label]) => [chunk.slice(start, end), label]);
    }
    knownNotes.set(chunk, foundNotes);
    if (foundNotes === null) return null;
    for (const [note, label] of foundNotes) {
      notes.push(note);
      labels.push(label);
    }
  }
  pieces.definitions = known;
  pieces.notes = knownNotes;
  // Footnotes number across the text: a piece that may call one renders after the calls
  // before it, with the labels the text defines (a call resolves only to a definition).
  // Without a definition, `[^x]` is text, as in a piece of its own.
  const footnoted = notes.length > 0;
  const defined = labels.join("\0");
  let calls = "";
  const usableBy = definitionsFor(pieces.references, chunks, definitions);
  const out: ReactNode[] = [];
  const counts = new Map<string, number>();
  const rendered: Piece[] = [];
  /** Root children so far, and whether the last one was removed (it rendered nothing). */
  let slots = 0;
  let barrier = false;
  /** Renders `source` as the piece at `index` (or takes the one rendered last time). */
  const take = (
    index: number,
    chunk: string,
    source: string,
    usable: string,
    last: boolean,
    section: boolean
  ): Piece | null => {
    const at = countsKey(counts);
    const footnotes = section || (footnoted && chunk.includes("[^")) ? `${defined}\u0001${calls}` : "";
    const piece: Piece | undefined = pieces.rendered[index];
    if (
      piece &&
      piece.chunk === chunk &&
      piece.definitions === usable &&
      piece.footnotes === footnotes &&
      piece.last === last &&
      piece.before === at &&
      (piece.incomplete === null || piece.incomplete === env.isIncomplete)
    ) {
      return piece;
    }
    const spine = Boolean(options.spine) && last && !section;
    let reader: HastReader | string;
    let after = calls;
    try {
      if (footnotes) {
        const result = rill.piece!(source, plan, { ...options, spine }, {
          defined: section ? "" : defined,
          calls,
          section,
        });
        reader = result.reader;
        after = result.calls;
      } else {
        reader = rill.markup!(source, plan, { ...options, spine });
      }
    } catch {
      return null;
    }
    if (typeof reader === "string") return null;
    const own = new Map(counts);
    const coded = { code: false };
    const nodes = segments(reader, env, own, coded);
    for (const [tag, count] of counts) own.set(tag, (own.get(tag) ?? 0) - count);
    // The section joins the root after the text's last node, as one more root child.
    if (section && typeof nodes[0] === "string" && nodes[0].startsWith("\n")) {
      nodes[0] = nodes[0].slice(1);
      if (nodes[0] === "") nodes.shift();
    }
    const incomplete = coded.code ? env.isIncomplete : null;
    return { chunk, definitions: usable, footnotes, calls: after, last, incomplete, before: at, nodes, counts: own };
  };
  /** Joins `piece` to the root's children, as its nodes sit in the whole text. */
  const place = (piece: Piece, removed: boolean, opens: boolean, streamingComment = false) => {
    rendered.push(piece);
    // A piece that cannot call a footnote passes the calls before it on, whatever they
    // were when it rendered.
    if (piece.footnotes) calls = piece.calls;
    for (const [tag, count] of piece.counts) counts.set(tag, (counts.get(tag) ?? 0) + count);
    // A piece of definitions alone has no node in the whole text's root; one whose
    // nodes sanitize removed (an HTML comment) had one, with a line ending on each side.
    if (piece.nodes.length === 0 && !removed) return;
    // The root's children join with a line ending, which merges with text beside it
    // (parse5 merges adjacent text: newlines it moves out of a table), but not across a
    // removed node (it was still there when parse5 ran).
    // At the root, text is only the line endings between nodes; one that a piece starts
    // (or ends) with sits after (or before) a node of its own that sanitize removed (an
    // HTML comment, a wrapper tag's start or end), which keeps it apart.
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
    // A comment still streaming at the text's end is gone before parse5 runs (see
    // `close_open_raw`): the line endings around it merge, as nothing kept them apart.
    barrier =
      (piece.nodes.length === 0 && !streamingComment) ||
      (typeof piece.nodes.at(-1) === "string" && />\s*$/.test(piece.chunk));
    for (let node = first; node < piece.nodes.length; node += 1) out.push(piece.nodes[node]);
  };
  for (let index = 0; index < chunks.length; index += 1) {
    const chunk = chunks[index]!;
    const usable = usableBy.get(chunk) ?? "";
    const piece = take(index, chunk, usable + chunk, usable, index === chunks.length - 1, false);
    if (!piece) return null;
    const removed =
      piece.nodes.length === 0 &&
      removedNode(chunk, [...(known.get(chunk) ?? []), ...(knownNotes.get(chunk) ?? []).map(([note]) => note)]);
    const last = index === chunks.length - 1;
    place(piece, removed, /^\s*</.test(chunk), last && /<!--(?![\s\S]*-->)/.test(chunk));
  }
  if (footnoted) {
    // The section: every footnote definition, with the link definitions their text may
    // use, after the calls of every piece.
    const source = [...definitions, ...notes].join("\n\n");
    const piece = take(chunks.length, source, source, "", false, true);
    if (!piece) return null;
    place(piece, false, true);
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
const VALUE_STYLE = 5;

/** `--name:value;...` (custom properties the animate plugin writes) as React's style. */
function styleObject(style: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const declaration of style.split(";")) {
    const colon = declaration.indexOf(":");
    out[declaration.slice(0, colon)] = declaration.slice(colon + 1);
  }
  return out;
}
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
      } else if (kind === VALUE_STYLE) {
        out[name] = styleObject(reader.string());
      }
    }
    return out;
  };
  const segment = (keyFor: (name: string) => string): ReactNode => {
    const kind = reader.next();
    if (kind === SEG_TEXT) return reader.string();
    if (kind === SEG_HAST || kind === SEG_MATH) {
      const node = reader.node();
      const nodes = env.settings.math ? mathOf(env)([node]) : [node];
      // Keyed as hast-util-to-jsx-runtime keys what KaTeX put in the math's place.
      const key = keyFor(nodes[0]?.tagName ?? node.tagName ?? "");
      if (kind === SEG_HAST) {
        return createElement(Fragment, { key }, jsxOf(nodes, env.components));
      }
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
  const math = env.settings.math;
  if (!math) return (nodes: HastNode[]) => nodes;
  let rewrite = mathByPlugin.get(math as object);
  if (!rewrite) {
    rewrite = mathFragments([math]);
    mathByPlugin.set(math as object, rewrite);
  }
  return rewrite as (nodes: HastNode[]) => HastNode[];
}

const katexCaches = new WeakMap<object, Map<string, string>>();

/** A math element (inside static HTML) as KaTeX's HTML. */
function katexHtml(node: HastNode, env: Env): string {
  const math = (env.settings.math as object | undefined) ?? NO_PLUGINS;
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
