// The example build's plugins in JavaScript: what Streamdown runs where rill does not,
// and what `checkEngine` holds the Rust forms (src/lib.rs) to.

type Node = {
  type: string;
  value?: string;
  tagName?: string;
  properties?: Record<string, unknown>;
  children?: Node[];
};

const EMOJI: Record<string, string> = { ":smile:": "😄", ":tada:": "🎉", ":rocket:": "🚀" };

/** `:smile:`, `:tada:` and `:rocket:` in text. */
export function remarkEmoji() {
  return (tree: Node) => {
    const visit = (node: Node) => {
      if (node.type === "text" && node.value?.includes(":")) {
        node.value = node.value.replace(/:(smile|tada|rocket):/g, (code) => EMOJI[code]!);
      }
      node.children?.forEach(visit);
    };
    visit(tree);
  };
}

/** Each text node outside links and code, replaced by what `split` makes of it. */
function editText(tree: Node, split: (text: string) => Node[] | null) {
  const visit = (node: Node) => {
    if (!node.children) return;
    node.children = node.children.flatMap((child) => {
      if (child.type === "text") return split(child.value ?? "") ?? [child];
      if (child.type === "element" && ["a", "code", "pre"].includes(child.tagName ?? "")) {
        return [child];
      }
      visit(child);
      return [child];
    });
  };
  visit(tree);
}

/** Splits `text` at `pattern`'s matches, making an element of each. */
function splitAt(text: string, pattern: RegExp, element: (match: RegExpExecArray) => Node) {
  const nodes: Node[] = [];
  let last = 0;
  for (const match of text.matchAll(pattern)) {
    const start = match.index + (match[1]?.length ?? 0);
    if (start > last) nodes.push({ type: "text", value: text.slice(last, start) });
    nodes.push(element(match));
    last = match.index + match[0].length;
  }
  if (nodes.length === 0) return null;
  if (last < text.length) nodes.push({ type: "text", value: text.slice(last) });
  return nodes;
}

/** `@name` links to `{base}/name`. Runs before rehype-sanitize. */
export function rehypeMentions(options: { base: string }) {
  return (tree: Node) =>
    editText(tree, (text) =>
      splitAt(text, /()@([A-Za-z0-9_]+)/g, (match) => ({
        type: "element",
        tagName: "a",
        properties: { href: `${options.base}/${match[2]}` },
        children: [{ type: "text", value: `@${match[2]}` }],
      }))
    );
}

/** `#tag` at the start of a text or after whitespace, as a tagged `<span>`. Runs last. */
export function rehypeHashtags() {
  return (tree: Node) =>
    editText(tree, (text) =>
      splitAt(text, /(^|[\t\n\f\r ])#([A-Za-z][A-Za-z0-9-]*)/g, (match) => ({
        type: "element",
        tagName: "span",
        properties: { className: ["hashtag"], "data-tag": match[2]!.toLowerCase() },
        children: [{ type: "text", value: `#${match[2]}` }],
      }))
    );
}
