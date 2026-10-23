// rill as Streamdown plugins. `rillPlugins` returns the `remarkPlugins` and
// `rehypePlugins` to hand `<Streamdown>`: rill parses and renders in Rust, the
// JavaScript tails finish raw HTML and math, and a document rill cannot take renders
// with the JavaScript pipeline it replaces. Either way the tree is the same.
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { defaultRehypePlugins, defaultRemarkPlugins } from "streamdown";
import { unified } from "unified";
import type { PluggableList, Processor } from "unified";

import type { HastNode, Rill, RillKind } from "./core.ts";
import { mathFragments } from "./math.ts";

/** Which pipeline rendered a document; `javascript` is the fallback. */
export type RenderKind = RillKind | "javascript";

/**
 * Documents longer than this (UTF-16 units) render in JavaScript. WebAssembly memory
 * grows to fit the largest document and never shrinks; this keeps it to tens of MB.
 */
export const DEFAULT_MAX_SOURCE_LENGTH = 128 * 1024;

type BaseOptions<A extends unknown[]> = {
  rill: Rill<A>;
  /**
   * The JavaScript pipeline rill replaces, which must be what its WebAssembly does:
   * the fallback, and (rehype) the tail for raw HTML. Defaults to Streamdown's.
   */
  remarkPlugins?: PluggableList;
  rehypePlugins?: PluggableList;
  /**
   * What runs on each math element of a `math` tree: KaTeX and the plugins after it
   * (the rest of the tree is already finished). Required with your own
   * `rehypePlugins`; defaults to Streamdown's otherwise.
   */
  mathTail?: PluggableList;
  /** Rehype plugins of your own, run on the finished tree. */
  after?: PluggableList;
  /** Longer documents render in JavaScript (see `DEFAULT_MAX_SOURCE_LENGTH`). */
  maxSourceLength?: number;
  /** Called once per render with the pipeline that produced it. */
  onRender?: (kind: RenderKind, ms: number, source: string) => void;
  /** Called when rill fails on a document; that document renders in JavaScript. */
  onError?: (error: unknown) => void;
};

/** `args` (the custom build's extra arguments) is required when its export takes any. */
export type RillPluginsOptions<A extends unknown[]> = BaseOptions<A> &
  ([] extends A ? { args?: A } : { args: A });

type HastRoot = {
  type: "root";
  children: unknown[];
  data?: unknown;
  position?: unknown;
};
type RillData = { kind: RenderKind; data: unknown };
type Point = { line: number; column: number; offset?: number };
type Position = { start: Point; end: Point };

// Streamdown caches processors by plugin name and JSON options, so each set carries an
// id, unique even across two copies of this package in one bundle.
const INSTANCE_PREFIX = Math.random().toString(36).slice(2);
let instances = 0;

/**
 * `remarkPlugins` and `rehypePlugins` for `<Streamdown>`. Pass them as they are (add
 * rehype plugins of your own with `after`, not by combining props), and memoize.
 */
export function rillPlugins<A extends unknown[]>(
  options: RillPluginsOptions<A>
) {
  const {
    rill,
    remarkPlugins = Object.values(defaultRemarkPlugins),
    rehypePlugins: customRehype,
    mathTail,
    after = [],
    maxSourceLength = DEFAULT_MAX_SOURCE_LENGTH,
    onRender,
    onError,
  } = options;
  if (customRehype && !mathTail) {
    throw new Error("rillPlugins: pass `mathTail` with your own `rehypePlugins`");
  }
  const rehypePlugins = customRehype ?? Object.values(defaultRehypePlugins);
  const args = ("args" in options ? (options.args ?? []) : []) as A;

  // Built on first use: most documents need neither.
  let javascript: Processor | undefined;
  let rawTail: Processor | undefined;
  let renderMath: ReturnType<typeof mathFragments> | undefined;

  function render(source: string): { kind: RenderKind; tree: HastRoot } {
    if (source.length <= maxSourceLength) {
      try {
        const result = rill.render(source, ...args);
        if (result) {
          return { kind: result.kind, tree: asHastRoot(result.tree) };
        }
      } catch (error) {
        onError?.(error);
      }
    }
    javascript ??= unified().use([
      remarkParse,
      ...remarkPlugins,
      [remarkRehype, { allowDangerousHtml: true }],
      ...rehypePlugins,
    ]) as Processor;
    return {
      kind: "javascript",
      tree: asHastRoot(javascript.runSync(javascript.parse(source), source)),
    };
  }

  function remarkRill(this: Processor) {
    this.parser = (source) => {
      const start = performance.now();
      const { kind, tree } = render(source);
      onRender?.(kind, performance.now() - start, source);
      const rillData: RillData = { kind, data: tree.data };
      // remark-rehype emits `hChildren` as the HAST root's children.
      return {
        type: "root" as const,
        children: [],
        ...(isPosition(tree.position) ? { position: tree.position } : {}),
        data: { hChildren: tree.children, rill: rillData },
      };
    };
    // Runs after plugins that replace the tree (a cache), so it carries its kind.
    return (
      tree: { data?: { rill?: RillData } },
      file: { data: Record<string, unknown> }
    ) => {
      file.data.rill = tree.data?.rill;
    };
  }

  function rehypeRill() {
    return (
      tree: HastRoot,
      file: { value?: unknown; data: Record<string, unknown> }
    ) => {
      const rillData = file.data.rill;
      // rill did not parse (its remark plugin was replaced): this is remark-rehype's
      // tree, so run the whole rehype chain on it, as the JavaScript pipeline would.
      const kind = isRillData(rillData) ? rillData.kind : "raw";
      if (kind === "raw") {
        // A cached root shares its children with remark-rehype; the tail edits in place.
        tree.children = structuredClone(tree.children);
        rawTail ??= unified().use(rehypePlugins) as Processor;
        const finished = asHastRoot(
          rawTail.runSync(tree as never, String(file.value ?? ""))
        );
        tree.children = finished.children;
        tree.data = finished.data;
        return;
      }
      if (kind === "math") {
        // Copies only what it changes, so a cached root stays as it was.
        renderMath ??= mathFragments(mathTail ?? defaultMathTail());
        tree.children = renderMath(tree.children as never);
      }
      if (isRillData(rillData) && rillData.data !== undefined) {
        tree.data = rillData.data;
      }
    };
  }

  const instance = `${INSTANCE_PREFIX}-${(instances += 1)}`;
  return {
    remarkPlugins: [[remarkRill, { instance }]] as PluggableList,
    rehypePlugins: [
      [rehypeRill, { instance }],
      ...after,
      // Off (`false`): Streamdown loads KaTeX's stylesheet only when its rehype-katex
      // is in the list; rill runs KaTeX itself.
      [attacherOf(defaultRehypePlugins.katex), false],
    ] as PluggableList,
  };
}

function attacherOf(plugin: unknown): unknown {
  return Array.isArray(plugin) ? plugin[0] : plugin;
}

/** Streamdown's rehype chain from KaTeX on: what each math element still needs. */
function defaultMathTail(): PluggableList {
  const { katex, sanitize, harden } = defaultRehypePlugins;
  return [katex, sanitize, harden].filter((plugin) => plugin !== undefined);
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
