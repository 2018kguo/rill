// rill as Streamdown plugins. `rillPlugins` returns the `remarkPlugins` and
// `rehypePlugins` to hand `<Streamdown>`: rill parses and renders in Rust, the
// JavaScript tail finishes raw HTML, Streamdown's own `plugins.math` renders KaTeX, and
// a document rill cannot take renders with the JavaScript pipeline it replaces. Either
// way the tree is the same.
import { defaultRehypePlugins, defaultRemarkPlugins } from "streamdown";
import type { PluginConfig } from "streamdown";
import { unified } from "unified";
import type { Pluggable, PluggableList, Processor } from "unified";

import type { HastNode, Rill, RillKind, Syntax } from "./core.ts";
import { mathFragments } from "./math.ts";

/** Which pipeline rendered a document; `javascript` is the fallback. */
export type RenderKind = RillKind | "javascript";

/**
 * Documents longer than this (UTF-16 units) render in JavaScript. WebAssembly memory
 * grows to fit the largest document and never shrinks; this keeps it to tens of MB.
 */
export const DEFAULT_MAX_SOURCE_LENGTH = 128 * 1024;

export type RillPluginsOptions = {
  /** rill for the build's arguments (`engine.rill(...args)`, or `loadRill()`). */
  rill: Rill;
  /**
   * The `plugins` you pass `<Streamdown>` (the same object): rill parses math and CJK
   * as they configure it, and renders each formula once with `plugins.math`'s KaTeX,
   * cached. Without them, rill renders documents only while `<Streamdown>` has no math
   * or CJK plugin either, and JavaScript renders the rest.
   */
  plugins?: Pick<PluginConfig, "math" | "cjk">;
  /**
   * The JavaScript pipeline rill replaces, which must be what its WebAssembly does:
   * Streamdown's `remarkPlugins` and `rehypePlugins` props. Defaults to Streamdown's.
   * Without rehype-raw, every document renders in JavaScript (Streamdown then shows
   * raw HTML as text, which rill does not do).
   */
  remarkPlugins?: PluggableList;
  rehypePlugins?: PluggableList;
  /** Rehype plugins of your own, run on the finished tree. */
  after?: PluggableList;
  /** Longer documents render in JavaScript (see `DEFAULT_MAX_SOURCE_LENGTH`). */
  maxSourceLength?: number;
  /** Called once per render with the pipeline that produced it. */
  onRender?: (kind: RenderKind, ms: number, source: string) => void;
  /** Called when rill fails on a document; that document renders in JavaScript. */
  onError?: (error: unknown) => void;
};


type HastRoot = {
  type: "root";
  children: unknown[];
  data?: unknown;
  position?: unknown;
};
type RillData = { kind: RenderKind; data: unknown };
type Point = { line: number; column: number; offset?: number };
type Position = { start: Point; end: Point };
type Attacher = (this: Processor, ...options: unknown[]) => unknown;
type Transformer = (tree: never, file: never) => unknown;

// Streamdown caches processors by plugin name and JSON options, so each set carries an
// id, unique even across two copies of this package in one bundle.
const INSTANCE_PREFIX = Math.random().toString(36).slice(2);
let instances = 0;

/**
 * `remarkPlugins` and `rehypePlugins` for `<Streamdown>`. Pass them as they are (add
 * rehype plugins of your own with `after`, not by combining props), and memoize.
 */
export function rillPlugins(options: RillPluginsOptions) {
  const {
    rill,
    plugins = {},
    remarkPlugins = Object.values(defaultRemarkPlugins),
    rehypePlugins = Object.values(defaultRehypePlugins),
    after = [],
    maxSourceLength = DEFAULT_MAX_SOURCE_LENGTH,
    onRender,
    onError,
  } = options;
  const raw = rehypePlugins.find(
    (plugin) => attacherOf(plugin) === attacherOf(defaultRehypePlugins.raw)
  );
  const syntax: Syntax = {
    math: plugins.math !== undefined,
    cjk: plugins.cjk !== undefined,
    // remark-math's own default is `$x$` math too (`@streamdown/math` turns it off).
    singleDollar:
      plugins.math !== undefined &&
      optionsOf(plugins.math.remarkPlugin)?.singleDollarTextMath !== false,
  };
  const supported = raw !== undefined;
  // Streamdown merges its `plugins` around these lists (index.tsx `mergedRemarkPlugins`):
  // what rill expects before and after its remark plugin.
  const expectedBefore = (plugins.cjk?.remarkPluginsBefore ?? []).map(
    attacherOf
  );
  const expectedAfter = [
    ...(plugins.cjk?.remarkPluginsAfter ?? []),
    ...(plugins.math ? [plugins.math.remarkPlugin] : []),
  ].map(attacherOf);

  // Built on first use: most documents need neither.
  let rawTail: Processor | undefined;
  let renderMath: ReturnType<typeof mathFragments> | undefined;

  function remarkRill(this: Processor) {
    const processor = this;
    // remark-parse's parser, which reads the extensions attached below at parse time.
    const remarkParser = processor.parser;
    // Streamdown's remark plugins, attached in their place: their syntax extensions
    // serve the JavaScript fallback, and their transforms run on its tree.
    const transformers = remarkPlugins.flatMap((plugin) => {
      const transformer = attach(processor, plugin);
      return typeof transformer === "function"
        ? [transformer as Transformer]
        : [];
    });
    let matches: boolean | undefined;

    function render(
      source: string,
      file: unknown
    ): { kind: RenderKind; tree: unknown } {
      matches ??= supported && configuredAsExpected(processor);
      if (matches && source.length <= maxSourceLength) {
        try {
          const result = rill.render(source, syntax);
          if (result) {
            return { kind: result.kind, tree: asHastRoot(result.tree) };
          }
        } catch (error) {
          onError?.(error);
        }
      }
      if (!remarkParser) {
        throw new Error(
          "rillPlugins: <Streamdown> runs remark-parse before rill"
        );
      }
      return { kind: "javascript", tree: remarkParser(source, file as never) };
    }

    processor.parser = (source, file) => {
      const start = performance.now();
      const { kind, tree } = render(source, file);
      onRender?.(kind, performance.now() - start, source);
      if (kind === "javascript") {
        // remark's own tree: the transforms and remark-rehype take it from here.
        return tree as never;
      }
      const root = tree as HastRoot;
      const rillData: RillData = { kind, data: root.data };
      // remark-rehype emits `hChildren` as the HAST root's children.
      return {
        type: "root" as const,
        children: [],
        ...(isPosition(root.position) ? { position: root.position } : {}),
        data: { hChildren: root.children, rill: rillData },
      };
    };
    // Runs after plugins that replace the tree (a cache), so it carries its kind.
    return (
      tree: { data?: { rill?: RillData } },
      file: { data: Record<string, unknown> }
    ) => {
      for (const transformer of transformers) {
        transformer(tree as never, file as never);
      }
      file.data.rill = tree.data?.rill;
    };
  }

  /** Whether the processor around rill's remark plugin is the pipeline it renders:
   * Streamdown's merge of exactly these `plugins`, and nothing else. */
  function configuredAsExpected(processor: Processor): boolean {
    const attachers = processor.attachers.map(([attacher]) => attacher);
    const at = attachers.indexOf(remarkRill as never);
    const rehype = attachers.indexOf(rehypeRill as never);
    // remark-parse comes first and remark-rehype just before rill's rehype plugin.
    const before = attachers.slice(1, at);
    const remarkAfter = attachers.slice(at + 1, rehype - 1);
    const same = (left: unknown[], right: unknown[]) =>
      left.length === right.length &&
      left.every((value, index) => value === right[index]);
    return (
      at > 0 &&
      rehype > at &&
      same(before, expectedBefore) &&
      same(remarkAfter, expectedAfter)
    );
  }

  function rehypeRill() {
    return (
      tree: HastRoot,
      file: { value?: unknown; data: Record<string, unknown> }
    ) => {
      const rillData = file.data.rill;
      // remark-rehype's tree (the JavaScript fallback, or rill's remark plugin was
      // replaced): run the whole rehype chain on it, as the JavaScript pipeline would.
      const kind =
        isRillData(rillData) && rillData.kind !== "javascript"
          ? rillData.kind
          : "raw";
      if (kind === "raw") {
        // A cached root shares its children with remark-rehype; the tail edits in place.
        tree.children = structuredClone(tree.children);
        rawTail ??= unified().use(rehypePlugins) as Processor;
        const finished = asHastRoot(
          rawTail.runSync(tree as never, String(file.value ?? ""))
        );
        tree.children = finished.children;
        tree.data = finished.data;
      } else if (isRillData(rillData) && rillData.data !== undefined) {
        tree.data = rillData.data;
      }
      // Streamdown's KaTeX runs next on whatever math is left: render each formula once,
      // cached, so it finds none. Copies only what it changes; a cached root stays.
      if (plugins.math) {
        renderMath ??= mathFragments([plugins.math.rehypePlugin]);
        tree.children = renderMath(tree.children as never);
      }
    };
  }

  const instance = `${INSTANCE_PREFIX}-${(instances += 1)}`;
  return {
    remarkPlugins: [[remarkRill, { instance }]] as PluggableList,
    rehypePlugins: [
      [rehypeRill, { instance }],
      ...after,
      // Off (`false`): Streamdown adds remarkEscapeHtml (raw HTML as text) unless it sees
      // rehype-raw; rill's tail runs it.
      ...(raw === undefined ? [] : [[attacherOf(raw), false]]),
    ] as PluggableList,
  };
}

/** Attaches `plugin` to `processor` as `use` would at this point; returns its
 * transformer. */
function attach(processor: Processor, plugin: Pluggable): unknown {
  const attacher = attacherOf(plugin);
  if (typeof attacher !== "function") {
    throw new Error(
      "rillPlugins: remarkPlugins must be plugins or [plugin, options]"
    );
  }
  const parameters = Array.isArray(plugin) ? plugin.slice(1) : [];
  if (parameters[0] === false) {
    return undefined;
  }
  return (attacher as Attacher).call(
    processor,
    ...(parameters[0] === true ? [] : parameters)
  );
}

function attacherOf(plugin: unknown): unknown {
  return Array.isArray(plugin) ? plugin[0] : plugin;
}

export function optionsOf(plugin: unknown): Record<string, unknown> | undefined {
  const options: unknown = Array.isArray(plugin) ? plugin[1] : undefined;
  return typeof options === "object" && options !== null
    ? (options as Record<string, unknown>)
    : undefined;
}

/** A processor's result, which for these pipelines is always a HAST root. */
function asHastRoot(node: HastNode | { type: string }): HastRoot {
  if (
    node.type !== "root" ||
    !("children" in node) ||
    !Array.isArray(node.children)
  ) {
    throw new Error(`Expected a HAST root, got ${node.type}`);
  }
  return {
    type: "root",
    children: node.children,
    ...("data" in node ? { data: node.data } : {}),
    ...("position" in node ? { position: node.position } : {}),
  };
}

function isRillData(value: unknown): value is RillData {
  return (
    typeof value === "object" &&
    value !== null &&
    "kind" in value &&
    typeof value.kind === "string"
  );
}

function isPosition(value: unknown): value is Position {
  const point = (side: unknown) =>
    typeof side === "object" &&
    side !== null &&
    "line" in side &&
    typeof side.line === "number" &&
    "column" in side &&
    typeof side.column === "number";
  return (
    typeof value === "object" &&
    value !== null &&
    "start" in value &&
    "end" in value &&
    point(value.start) &&
    point(value.end)
  );
}
