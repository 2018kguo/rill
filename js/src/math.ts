// The math tail: rill returns a `math` tree finished except for its math elements, which
// are left as sanitized `<code class="language-math">` (in a `<pre>` for display math).
// Each is rendered on its own: the tail's plugins (Streamdown's KaTeX, sanitize and
// harden) run on a tree holding just that element, and the result takes its place.
// conformance/diff.mjs checks this against Streamdown's whole-tree order.
// rehype-katex only replaces that element and sanitize and harden act per element, so
// this matches running them on the whole tree. Results are cached by expression, so a
// streamed answer renders each formula once, not on every update.
import { unified } from "unified";
import type { PluggableList, Processor } from "unified";

type Node = {
  type: string;
  tagName?: string;
  properties?: Record<string, unknown>;
  children?: Node[];
  value?: string;
};

/** Distinct expressions kept per tail before the cache starts over. */
const CACHE_LIMIT = 1024;

/** Renders each math element in a list of nodes with `plugins`, cached. */
export function mathFragments(plugins: PluggableList): (nodes: Node[]) => Node[] {
  let processor: Processor | undefined;
  const cache = new Map<string, Node[]>();

  function render(scope: Node): Node[] {
    const key = `${scope.tagName}\0${classes(scope).join(" ")}\0${text(scope)}`;
    let rendered = cache.get(key);
    if (rendered === undefined) {
      processor ??= unified().use(plugins) as Processor;
      const root = { type: "root", children: [structuredClone(scope)] };
      rendered = (processor.runSync(root as never) as unknown as Node).children ?? [];
      if (cache.size >= CACHE_LIMIT) cache.clear();
      cache.set(key, rendered);
    }
    return rendered.map(copy);
  }

  /** `nodes` with each math element rendered; the same array when none holds math.
   * Never edits its input, which a cached tree may share. */
  function rewrite(nodes: Node[]): Node[] {
    let out: Node[] | undefined;
    for (const [index, node] of nodes.entries()) {
      let replacement: Node[] | undefined;
      if (node.type === "element") {
        const scope = mathScope(node);
        if (scope !== undefined) {
          replacement = render(scope);
        } else if (node.children) {
          const children = rewrite(node.children);
          if (children !== node.children) {
            replacement = [{ ...node, children }];
          }
        }
      }
      if (replacement !== undefined) {
        out ??= nodes.slice(0, index);
        out.push(...replacement);
      } else if (out !== undefined) {
        out.push(node);
      }
    }
    return out ?? nodes;
  }

  return rewrite;
}

/** `mathFragments` as a rehype plugin, for running it with unified. */
export function rehypeMathFragments(plugins: PluggableList) {
  const rewrite = mathFragments(plugins);
  return () => (tree: Node) => ({ ...tree, children: rewrite(tree.children ?? []) });
}

/** The element rehype-katex replaces for `element`: itself, or the `<pre>` around a
 * ```math block's `<code>`. `undefined` when it holds no math. */
function mathScope(element: Node): Node | undefined {
  if (isMath(element)) return element;
  const code =
    element.tagName === "pre"
      ? element.children?.find((child) => child.type === "element" && child.tagName === "code")
      : undefined;
  return code && classes(code).includes("language-math") ? element : undefined;
}

function isMath(node: Node) {
  const list = classes(node);
  return (
    list.includes("language-math") || list.includes("math-display") || list.includes("math-inline")
  );
}

function classes(node: Node): unknown[] {
  const className = node.properties?.className;
  return Array.isArray(className) ? className : [];
}

function text(node: Node): string {
  return node.type === "text" ? (node.value ?? "") : (node.children ?? []).map(text).join("");
}

/** A deep copy of a HAST node: later plugins may edit the tree, never the cache. Plain
 * data, so much faster than structuredClone on KaTeX's deep span trees. */
function copy(node: Node): Node {
  const out: Node = { ...node };
  if (node.properties) {
    out.properties = {};
    for (const [name, value] of Object.entries(node.properties)) {
      out.properties[name] = Array.isArray(value) ? [...value] : value;
    }
  }
  if (node.children) out.children = node.children.map(copy);
  return out;
}
