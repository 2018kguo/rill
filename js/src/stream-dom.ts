// The DOM half of rill-stream: keeps a root element's children equal to Streamdown's
// blocks for a streaming text. Final blocks stay as they are; each update applies the
// session's edit to the open tail.
//
// Growing text gets a new text node after the old one rather than appendData, which
// copies the whole string and costs about 20 times more in Chromium. The HTML is the same;
// while a block streams, a run of adjacent text nodes stands for the one text node its
// HTML parses to, and the block is normalize()d into exactly that once it is final.

/** rill-stream's WebAssembly session (`Stream` from its wasm-bindgen glue). */
export type StreamSession = {
  /** 0: patch, 1: reset, 2: fall back. */
  update(appended: string, reset: boolean): number;
  /** After a reset: the whole HTML. */
  readonly html: string;
  /** After a patch: the edit (crates/rill-stream/src/diff.rs). */
  readonly ops: Uint32Array;
  readonly text: string;
  /** Root nodes that became final this update. */
  readonly finalNodes: number;
  free(): void;
};

const PATCH = 0;
const FALLBACK = 2;
const APPEND = 0;
const REMOVE = 1;
const INSERT = 2;

export type StreamRenderer = {
  /** Shows `text`; `false` when it needs the exact path (the root is left as it was). */
  update(text: string): boolean;
  dispose(): void;
};

export function createStreamRenderer(root: Element, session: StreamSession): StreamRenderer {
  let text = "";
  /** Root children before the open tail. */
  let finals = 0;
  return {
    update(next) {
      // V8 compares a slice with memcmp; startsWith reads each character.
      const grows = next.length >= text.length && next.slice(0, text.length) === text;
      const outcome = grows ? session.update(next.slice(text.length), false) : session.update(next, true);
      text = next;
      if (outcome === FALLBACK) return false;
      if (outcome === PATCH) {
        apply(root, finals, session.ops, session.text);
      } else {
        root.innerHTML = session.html;
        finals = 0;
      }
      for (let added = session.finalNodes; added > 0; added -= 1) {
        root.childNodes[finals]!.normalize();
        finals += 1;
      }
      return true;
    },
    dispose() {
      session.free();
    },
  };
}

/** The node `index` places into `parent`, counting a run of adjacent text nodes as one. */
function child(parent: Node, index: number): ChildNode | undefined {
  let node = parent.firstChild;
  for (; node !== null && index > 0; index -= 1) {
    if (node.nodeType === 3) while (node.nextSibling?.nodeType === 3) node = node.nextSibling;
    node = node.nextSibling;
  }
  return node ?? undefined;
}

/** Runs an edit program against the tail of `root` (from child `finals` on). */
function apply(root: Element, finals: number, ops: Uint32Array, strings: string) {
  const document = root.ownerDocument;
  let at = 0;
  while (at < ops.length) {
    const opcode = ops[at]!;
    const depth = ops[at + 1]!;
    at += 2;
    // The node at the path (depth 0: the root itself); a REMOVE's path may end one past
    // its parent's last child. Blocks at the root are elements: index them directly.
    let node: Node | undefined = root;
    const chain: Node[] = [];
    for (let level = 0; level < depth; level += 1) {
      chain.push(node!);
      node = level === 0 ? root.childNodes[finals + ops[at]!] : child(node!, ops[at + level]!);
    }
    at += depth;
    if (opcode === REMOVE) {
      // The node, its later siblings, and the later siblings of each ancestor.
      let level = depth;
      let from = (node ?? null) as ChildNode | null;
      while (level > 0) {
        const parent = chain[level - 1]!;
        while (from !== null) {
          const after: ChildNode | null = from.nextSibling;
          parent.removeChild(from);
          from = after;
        }
        level -= 1;
        from = level > 0 ? (chain[level]!.nextSibling as ChildNode | null) : null;
      }
      continue;
    }
    const value = strings.slice(ops[at]!, ops[at + 1]!);
    at += 2;
    if (opcode === APPEND) {
      let last = node as Node;
      while (last.nextSibling?.nodeType === 3) last = last.nextSibling;
      last.parentNode!.insertBefore(document.createTextNode(value), last.nextSibling);
    } else if (opcode === INSERT) build(node as Element, value);
    else (node as Element).append(value);
  }
}

/** Each distinct open tag rill-stream emits, parsed once; new elements are clones. */
const prototypes = new Map<string, Element>();
const VOID = /^<(?:br|hr|img|input)[ >]/;
const ENTITY = /&(amp|lt|gt|quot);/g;
const ENTITIES: Record<string, string> = { amp: "&", lt: "<", gt: ">", quot: '"' };

/**
 * Appends `html` (rill-stream's own HTML: quoted attributes, the entities above) to
 * `parent`. Cloning a cached element is several times cheaper than the HTML parser for
 * the short fragments a stream inserts.
 */
function build(parent: Element, html: string) {
  const document = parent.ownerDocument;
  let node: Node = parent;
  let at = 0;
  while (at < html.length) {
    if (html.charCodeAt(at) !== 60 /* < */) {
      let end = html.indexOf("<", at);
      if (end < 0) end = html.length;
      const text = html.slice(at, end);
      node.appendChild(document.createTextNode(text.includes("&") ? text.replace(ENTITY, (_, name: string) => ENTITIES[name]!) : text));
      at = end;
      continue;
    }
    if (html.charCodeAt(at + 1) === 47 /* / */) {
      node = node.parentNode!;
      at = html.indexOf(">", at) + 1;
      continue;
    }
    // Attribute values are quoted and may hold `>`.
    let end = at + 1;
    for (let quoted = false; ; end += 1) {
      const code = html.charCodeAt(end);
      if (code === 34) quoted = !quoted;
      else if (code === 62 && !quoted) break;
    }
    const tag = html.slice(at, end + 1);
    let prototype = prototypes.get(tag);
    if (prototype === undefined) {
      const template = document.createElement("template");
      template.innerHTML = tag;
      prototype = template.content.firstChild as Element;
      prototypes.set(tag, prototype);
    }
    const element = node.appendChild(prototype.cloneNode(false));
    if (!VOID.test(tag)) node = element;
    at = end + 1;
  }
}
