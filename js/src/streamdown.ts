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

import { CodeSegment } from "./code-block.ts";
import type { HastNode, HastReader, Rill } from "./core.ts";
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
export { setHighlightWorker } from "./highlight.ts";

/** What `Block` needs from `<Streamdown>`'s props that its own props lack. */
type Settings = {
  plugins: PluginConfig | undefined;
  cn: Cn;
  prefix: string;
  /** `fallbackComponent` is set: every element renders through Streamdown. */
  fallback: boolean;
  highlight: Highlighter | undefined;
};

const RillContext = createContext<Settings | null>(null);

/** How many block renders rill took and how many went to Streamdown's own Block
 * (diagnostics, for tests and benchmarks). */
export const blockRenders = { rill: 0, streamdown: 0 };

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
  const { plugins, prefix, fallbackComponent } = props;
  const settings = useMemo<Settings>(
    () => ({
      plugins,
      cn: createCn(prefix),
      prefix: prefix ?? "",
      fallback: fallbackComponent !== undefined,
      highlight: highlighterFor(plugins?.code),
    }),
    [plugins, prefix, fallbackComponent]
  );
  useEffect(load, []);
  return createElement(
    RillContext.Provider,
    { value: settings },
    createElement(StreamdownComponent, {
      ...props,
      BlockComponent: props.BlockComponent ?? Block,
    })
  );
}, sameStreamdownProps as never);
(Streamdown as { displayName?: string }).displayName = "Streamdown";

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
  const { linkSafety } = useContext(StreamdownContext);
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
  const nodes = useMemo(() => {
    if (!(plan && settings && rill?.markup) || content.length > DEFAULT_MAX_SOURCE_LENGTH) {
      return null;
    }
    let reader: HastReader | null = null;
    try {
      reader = rill.markup(content, plan, {
        prefix: settings.prefix,
        linkSafety: safe,
        ssr,
        overrides: plan.overrides,
      });
    } catch {
      return null;
    }
    if (!reader) return null;
    return segments(reader, {
      settings,
      components: props.components as Record<string, unknown>,
      isIncomplete: props.isIncomplete,
      ssr,
      openLink,
    });
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

/** One block's segments as React nodes. */
function segments(reader: HastReader, env: Env): ReactNode[] {
  const list = (): ReactNode[] => {
    const count = reader.next();
    const out = new Array<ReactNode>(count);
    // hast-util-to-jsx-runtime's keys: `tag-n`, counting each tag among siblings.
    const counts = new Map<string, number>();
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
  return list();
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
