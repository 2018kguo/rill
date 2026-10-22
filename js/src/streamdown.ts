// rill as Streamdown plugins. `rillPlugins` returns the `remarkPlugins` and
// `rehypePlugins` to hand `<Streamdown>`: rill parses and renders in Rust, the
// JavaScript tails finish raw HTML and math, and a document rill cannot take renders
// with the JavaScript pipeline it replaces. Either way the tree is the same.
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { defaultRehypePlugins, defaultRemarkPlugins } from "streamdown";
import { unified } from "unified";
import type { PluggableList, Processor } from "unified";

import type { HastNode, Rill, RillKind } from "./index.ts";

/** Which pipeline rendered a document; `javascript` is the fallback. */
export type RenderKind = RillKind | "javascript";

export type RillPluginsOptions<A extends unknown[]> = {
  rill: Rill<A>;
  /** Extra arguments for a custom build's render export. */
  args?: A;
  /**
   * The JavaScript pipeline rill replaces, which must be what its WebAssembly does:
   * the fallback, and (rehype) the tail for raw HTML. Defaults to Streamdown's.
   */
  remarkPlugins?: PluggableList;
  rehypePlugins?: PluggableList;
  /**
   * What still runs on a `math` tree: KaTeX and the plugins after it. Required with
   * your own `rehypePlugins`; defaults to Streamdown's otherwise.
   */
  mathTail?: PluggableList;
  /** Plugins of your own, run after the pipeline on its result. */
  after?: { remark?: PluggableList; rehype?: PluggableList };
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

let instances = 0;

/** `remarkPlugins` and `rehypePlugins` for `<Streamdown>`; memoize the result. */
export function rillPlugins<A extends unknown[]>({
  rill,
  args,
  remarkPlugins = Object.values(defaultRemarkPlugins),
  rehypePlugins: customRehype,
  mathTail,
  after,
  onRender,
  onError,
}: RillPluginsOptions<A>) {
  if (customRehype && !mathTail) {
    throw new Error("rillPlugins: pass `mathTail` with your own `rehypePlugins`");
  }
  const rehypePlugins = customRehype ?? Object.values(defaultRehypePlugins);
  const rillArgs = (args ?? []) as A;
  const javascript = unified().use([
    remarkParse,
    ...remarkPlugins,
    [remarkRehype, { allowDangerousHtml: true }],
    ...rehypePlugins,
  ]);
  const tails = {
    raw: unified().use(rehypePlugins),
    math: unified().use(mathTail ?? defaultMathTail()),
  };

  function render(source: string): { kind: RenderKind; tree: HastRoot } {
    try {
      const result = rill.render(source, ...rillArgs);
      if (result) {
        return { kind: result.kind, tree: asHastRoot(result.tree) };
      }
    } catch (error) {
      onError?.(error);
    }
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
    // Runs after plugins that replace the tree, so a cached tree carries its kind.
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
      if (!isRillData(rillData)) {
        return;
      }
      if (rillData.kind === "raw" || rillData.kind === "math") {
        // A cached root shares its children with remark-rehype; KaTeX edits in place.
        tree.children = structuredClone(tree.children);
        const tail = tails[rillData.kind];
        const finished = asHastRoot(
          tail.runSync(tree as never, String(file.value ?? ""))
        );
        tree.children = finished.children;
        tree.data = finished.data;
      } else if (rillData.data !== undefined) {
        tree.data = rillData.data;
      }
    };
  }

  // Streamdown caches processors by plugin name and JSON options, so each instance
  // carries its own id.
  const instance = (instances += 1);
  return {
    remarkPlugins: [
      [remarkRill, { instance }],
      ...(after?.remark ?? []),
    ] as PluggableList,
    rehypePlugins: [
      [rehypeRill, { instance }],
      ...(after?.rehype ?? []),
    ] as PluggableList,
  };
}

/** Streamdown's rehype chain from KaTeX on: what a `math` tree still needs. */
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
