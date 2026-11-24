// The DOM half of rill-stream: keeps a root element's children equal to Streamdown's
// blocks for a streaming text. Final blocks stay as they are; each update applies the
// session's edit to the open tail.
//
// The module renders a token stream (crates/rill-stream/src/tokens.rs): a `u32` per
// element open, close or text run, the text in UTF-16. Elements are clones of one
// parsed element per tag; the edit (src/diff.rs) points into the tokens in place. Text
// that is a stretch of the text as given is sliced from the string itself; only the
// rest is read out of the module's memory.
//
// Growing text gets a new text node after the old one rather than appendData, which
// copies the whole string and costs about 20 times more in Chromium. The HTML is the same;
// a run of adjacent text nodes stands for the one text node it parses to (call
// normalize() on the root for exactly Streamdown's nodes).

/** rill-stream's WebAssembly session (`Stream` from its wasm-bindgen glue). */
export type StreamSession = {
  /** An input buffer of at least `length` UTF-16 units, as a `u16` index into memory. */
  inputAt(length: number): number;
  /** Renders the text so far plus the buffer's first `length` units (with `reset`, they
   * are the whole text). 0: patch, 1: reset, 2: fall back. */
  updateInput(length: number, reset: boolean): number;
  /** Where the update's header is, as a `u32` index into memory: the outcome; the edit's
   * opcodes, as a `u32` index and count; the rendering's tokens, the same; its text, as a
   * `u16` index and count; the first text unit the edit refers to; the root nodes that
   * became final; the tokens' origins, as a `u32` index (one a token); how far the
   * rendered text equals the text as given; how many strings are interned. */
  headerAt(): number;
  /** The interned strings from `from` on, each ended by a NUL. */
  strings(from: number): string;
  free(): void;
};

/** The module's exports the renderer needs besides a session. */
export type StreamModule = {
  /** The module's memory (`streamMemory()`). */
  memory: WebAssembly.Memory;
  /** The tag ids' open tags, one a line (`streamTags()`). */
  tags: string;
};

const PATCH = 0;
const FALLBACK = 2;
const APPEND = 0;
const REMOVE = 1;
const RETAG = 3;
const OPEN = 0;
const CLOSE = 1;
const VOID = 2;
const TEXT_KIND = 3;
/** Tag ids from here on: interned strings (an open tag, or a whole subtree as `VOID`). */
const INTERNED = 64;
/** An APPEND range in the text as given. */
const SOURCE = 0x80000000;
/** A text token that is not one stretch of the text as given. */
const NOWHERE = 0xffffffff;

export type StreamRenderer = {
  /** Shows `text`; `false` when it needs the exact path (the root is left as it was). */
  update(text: string): boolean;
  /** The islands in the DOM now (see `Island`), in document order. */
  islands(): Island[];
  dispose(): void;
};

/** An element of a tag the page's `components` override: rendered by React, from the
 * tokens, in place of a comment that holds its position (see `holder`). */
export type Island = {
  /** Stable for the island's life (a React key). */
  key: number;
  anchor: Comment;
  /** The island's element and subtree, as `IslandNode`s; `json` compares them. */
  tree: IslandElement;
  json: string;
  /** The portal container React renders the island into (see `container`). */
  holder: Element;
};

/** An element of an island's subtree: its open tag's HTML (attributes and all), and
 * for a static subtree (`VOID`) its inner HTML. */
export type IslandElement = {
  html: string;
  inner?: string;
  island: boolean;
  children: IslandNode[];
};
export type IslandNode = IslandElement | string;

/** What an island's interned open tag starts with. */
const ISLAND = "\u0001";

/** Nodes React put in place for an island: not the stream's (positions skip them). */
const owned = new WeakSet<Node>();

/** Whether `node` is one an island's component rendered (at its top). */
export const isIslandNode = (node: Node): boolean => owned.has(node);

/** Each anchor's island. */
const islandOf = new WeakMap<Comment, Island>();
let islandKeys = 0;

/** Each island's anchor and the nodes React put after it, by the island's key. */
const placed = new Map<string, { anchor: Comment; nodes: Node[] }>();
/** Elements whose children React's islands insert (see `container`). */
const patched = new WeakSet<Node>();

/** The island (its key) whose portal holds `node`: the React fiber React keeps on each
 * node it made, up to its portal (whose key is the island's). */
type Fiber = { tag: number; key: string | null; return: Fiber | null };
function portalKey(node: Node): string | null {
  const name = Object.keys(node).find((name) => name.startsWith("__reactFiber$"));
  // (A HostPortal fiber's tag is 4.)
  let fiber = name === undefined ? null : ((node as unknown as Record<string, Fiber>)[name] ?? null);
  while (fiber !== null && fiber.tag !== 4) fiber = fiber.return;
  return fiber?.key ?? null;
}

/**
 * The portal container for the island `key` at `anchor`: the anchor's parent element,
 * so the island's nodes are where React's events reach them. React's inserts into it
 * go right after the island's anchor (and its nodes before them), and other code's
 * edits (the stream's own) are left as they are.
 */
function container(anchor: Comment, key: string): Element {
  const parent = anchor.parentNode as Element;
  placed.set(key, { anchor, nodes: [] });
  if (!patched.has(parent)) {
    patched.add(parent);
    const { insertBefore, removeChild } = parent;
    const route = (node: Node, before: Node | null) => {
      const key = before === null ? portalKey(node) : null;
      const island = key === null ? undefined : placed.get(key);
      if (island === undefined) return insertBefore.call(parent, node, before);
      owned.add(node);
      const { anchor, nodes } = island;
      const at = nodes.indexOf(node);
      if (at >= 0) nodes.splice(at, 1);
      const last = nodes.findLast((n) => n.parentNode === anchor.parentNode) ?? anchor;
      nodes.push(node);
      return insertBefore.call(anchor.parentNode, node, last.nextSibling);
    };
    Object.assign(parent, {
      appendChild: (node: Node) => route(node, null),
      insertBefore: (node: Node, before: Node | null) => {
        // A node of an island goes where its siblings are.
        if (before !== null && owned.has(before)) owned.add(node);
        return before === null ? route(node, null) : insertBefore.call(parent, node, before);
      },
      // (The stream may have removed it with its parent already.)
      removeChild: (node: Node) => (node.parentNode === parent ? removeChild.call(parent, node) : node),
    });
  }
  return parent;
}

/** Per document: the tags' elements (per module's list) and each parsed open tag or
 * subtree, by its HTML (they are the same across messages). */
const parsed = new WeakMap<
  Document,
  { tags: Map<string, Element[]>; html: Map<string, Node>; template: HTMLTemplateElement }
>();

function parsedFor(document: Document) {
  let cache = parsed.get(document);
  if (cache === undefined) {
    cache = { tags: new Map(), html: new Map(), template: document.createElement("template") };
    parsed.set(document, cache);
  }
  return cache;
}

export function createStreamRenderer(
  root: Element,
  session: StreamSession,
  module: StreamModule,
  options: { islands?: boolean } = {},
): StreamRenderer {
  const { memory } = module;
  const document = root.ownerDocument;
  const cache = parsedFor(document);
  const { template } = cache;
  let tags = cache.tags.get(module.tags);
  if (tags === undefined) {
    tags = module.tags.split("\n").map((tag) => {
      template.innerHTML = tag;
      return template.content.firstChild as Element;
    });
    cache.tags.set(module.tags, tags);
  }
  const utf16 = new TextDecoder("utf-16le");
  const state: Root = {
    islands: [],
    element: root,
    tags,
    finals: 0,
    count: 0,
    u32: new Uint32Array(0),
    input: "",
    origins: 0,
    interned: [],
    nodes: [],
    template,
    parsed: cache.html,
    literal: (start, end) => {
      // Decoded once an update, on first use.
      if (state.strings === undefined) {
        const units = state.u32[header + 6]! - state.from;
        const at = state.u32[header + 5]! + state.from;
        state.strings = utf16.decode(u16.subarray(at, at + units));
      }
      return state.strings.slice(start - state.from, end - state.from);
    },
    strings: undefined,
    from: 0,
  };
  const withIslands = options.islands === true;
  const tagHtml = module.tags.split("\n");
  /** The islands of the rendering (`words`, `count` tokens: the open tail) read from its
   * tokens, paired in order with the tail's anchors; an unchanged one is kept as it was. */
  const rescan = (words: number, count: number) => {
    const { u32 } = state;
    const base = u32[header + 5]!;
    let literal = 0;
    const read = (at: number): [IslandElement, number] => {
      const id = u32[at]! >>> 2;
      const html = id < INTERNED ? tagHtml[id]! : state.interned[id - INTERNED]!;
      const element: IslandElement = {
        html: html.replace(ISLAND, ""),
        island: html.startsWith(ISLAND),
        children: [],
      };
      for (at += 1; ; at += 1) {
        const word = u32[at]!;
        const kind = word & 3;
        if (kind === CLOSE) return [element, at];
        if (kind === OPEN) {
          const [inner, end] = read(at);
          element.children.push(inner);
          at = end;
        } else if (kind === VOID) {
          const id = word >>> 2;
          const html = id < INTERNED ? tagHtml[id]! : state.interned[id - INTERNED]!;
          element.children.push({ html, inner: "", island: false, children: [] });
        } else {
          const length = word >>> 2;
          const origin = u32[state.origins + at]!;
          if (origin !== NOWHERE) element.children.push(state.input.slice(origin, origin + length));
          else {
            element.children.push(
              utf16.decode(u16.subarray(base + literal, base + literal + length)),
            );
            literal += length;
          }
        }
      }
    };
    const trees: IslandElement[] = [];
    for (let at = words; at < words + count; at += 1) {
      const word = u32[at]!;
      const kind = word & 3;
      if (kind === TEXT_KIND) {
        if (u32[state.origins + at]! === NOWHERE) literal += word >>> 2;
      } else if (
        kind === OPEN &&
        word >>> 2 >= INTERNED &&
        state.interned[(word >>> 2) - INTERNED]!.startsWith(ISLAND)
      ) {
        const [tree, end] = read(at);
        trees.push(tree);
        at = end;
      }
    }
    // The tail's anchors, in document order (not inside islands' own nodes).
    const anchors: Comment[] = [];
    const walk = (node: ChildNode | null) => {
      for (; node !== null; node = node.nextSibling) {
        if (owned.has(node)) continue;
        if (node.nodeType === 8) anchors.push(node as Comment);
        else if (node.nodeType === 1) walk(node.firstChild);
      }
    };
    walk(state.count > state.finals ? tailChild(state, 0) : null);
    if (anchors.length === 0 && state.islands.length === 0) return;
    const tail = new Set(anchors);
    const islands = state.islands.filter(
      (island) => island.anchor.isConnected && !tail.has(island.anchor),
    );
    anchors.forEach((anchor, index) => {
      const tree = trees[index]!;
      const json = JSON.stringify(tree);
      let island = islandOf.get(anchor);
      // (An anchor moved into another element, by a retag, starts over there.)
      if (island === undefined || island.holder !== anchor.parentNode) {
        island = {
          key: (islandKeys += 1),
          anchor,
          tree,
          json,
          holder: container(anchor, String(islandKeys)),
        };
        islandOf.set(anchor, island);
      } else if (island.json !== json) {
        island = { ...island, tree, json };
        islandOf.set(anchor, island);
      }
      islands.push(island);
    });
    if (
      islands.length !== state.islands.length ||
      islands.some((island, index) => island !== state.islands[index])
    ) {
      state.islands = islands;
    }
  };
  let text = "";
  /** Views of all of memory, made again only when it grows (which detaches them). */
  let u16 = new Uint16Array(0);
  let input = 0;
  let inputLength = 0;
  let header = 0;
  const views = () => {
    if (state.u32.buffer !== memory.buffer) {
      state.u32 = new Uint32Array(memory.buffer);
      u16 = new Uint16Array(memory.buffer);
    }
  };
  /** Writes `next[from..]` into the module's input buffer: its length. */
  const write = (next: string, from: number) => {
    const length = next.length - from;
    if (inputLength < length) {
      inputLength = Math.max(length, 2 * inputLength, 4096);
      input = session.inputAt(inputLength);
    }
    views();
    for (let at = 0; at < length; at += 1) u16[input + at] = next.charCodeAt(from + at);
    return length;
  };
  return {
    update(next) {
      // V8 compares a slice with memcmp; startsWith reads each character.
      const grows = next.length >= text.length && next.slice(0, text.length) === text;
      const outcome = grows
        ? session.updateInput(write(next, text.length), false)
        : session.updateInput(write(next, 0), true);
      text = next;
      if (outcome === FALLBACK) return false;
      views();
      header ||= session.headerAt();
      if (state.u32[header + 11]! > state.interned.length) {
        const added = session.strings(state.interned.length).split("\0");
        for (let at = 0; at < added.length - 1; at += 1) state.interned.push(added[at]!);
        // Passing the strings out may grow memory, which detaches the views.
        views();
      }
      const u32 = state.u32;
      const words = u32[header + 3]!;
      // Only the text the edit refers to: from its first unit on.
      state.from = u32[header + 7]!;
      state.strings = undefined;
      state.input = next;
      state.origins = u32[header + 9]! - words;
      if (outcome === PATCH) {
        apply(state, u32[header + 1]!, u32[header + 2]!, words);
      } else {
        root.textContent = "";
        state.finals = 0;
        state.from = 0;
        state.count = build(state, root, words, words + u32[header + 4]!, 0);
      }
      if (withIslands) rescan(words, u32[header + 4]!);
      state.finals += u32[header + 8]!;
      return true;
    },
    islands: () => state.islands,
    dispose() {
      session.free();
    },
  };
}

/** The DOM side's state. */
type Root = {
  /** The islands, in document order. */
  islands: Island[];
  element: Element;
  /** An element of each tag id, to clone. */
  tags: Element[];
  /** Root children before the open tail, and in all. */
  finals: number;
  count: number;
  /** All of memory. */
  u32: Uint32Array;
  /** The text as given. */
  input: string;
  /** The interned strings, and each one's node once parsed (see `clone`). */
  interned: string[];
  nodes: (Node | undefined)[];
  template: HTMLTemplateElement;
  /** Parsed open tags and subtrees by their HTML (shared across the document's roots). */
  parsed: Map<string, Node>;
  /** From a token's index to its origin's. */
  origins: number;
  /** The rendering's text `[start, end)`. */
  literal(start: number, end: number): string;
  /** The rendering's text from unit `from` on, once decoded. */
  strings: string | undefined;
  from: number;
};

/** The node `index` places into `parent`, counting a run of adjacent text nodes as one. */
function child(parent: Node, index: number): ChildNode | null {
  let node = skip(parent.firstChild);
  for (; node !== null && index > 0; index -= 1) {
    if (node.nodeType === 3)
      while (node.nextSibling?.nodeType === 3 && !owned.has(node.nextSibling))
        node = node.nextSibling;
    node = skip(node.nextSibling);
  }
  return node;
}

/** `node`, or the first node after it that is not an island's. */
function skip(node: ChildNode | null): ChildNode | null {
  while (node !== null && owned.has(node)) node = node.nextSibling;
  return node;
}

/** The first node of the run of text nodes `node` is in (or `node`). */
function runStart(node: ChildNode | null): ChildNode | null {
  while (node !== null && owned.has(node)) node = node.previousSibling;
  if (node?.nodeType === 3)
    while (node.previousSibling?.nodeType === 3 && !owned.has(node.previousSibling))
      node = node.previousSibling;
  return node;
}

/** The tail's root child `index` (from the end: the tail is the last block or two),
 * counting a run of adjacent text nodes as one (an append adds a text node). */
function tailChild(root: Root, index: number): ChildNode | null {
  let node = runStart(root.element.lastChild);
  for (let steps = root.count - root.finals - 1 - index; steps > 0 && node !== null; steps -= 1)
    node = runStart(node.previousSibling);
  return node;
}

/** Runs the edit program at `u32[ops..ops + length]` against the open tail. */
function apply(root: Root, ops: number, length: number, words: number) {
  const { u32, element } = root;
  const document = element.ownerDocument;
  const end = ops + length;
  let at = ops;
  while (at < end) {
    const opcode = u32[at]!;
    const depth = u32[at + 1]!;
    at += 2;
    // The node at the path (depth 0: the root itself) and its parent; a REMOVE's path
    // may end one past its parent's last child.
    let parent: Node = element;
    let node: Node | null = element;
    // An edit inside an island (its anchor on the path, or the anchor itself retagged):
    // the island is read from the tokens again after the edit.
    let inside = false;
    for (let level = 0; level < depth; level += 1) {
      parent = node!;
      if (parent.nodeType === 8) {
        inside = true;
        // A REMOVE in it also removes what follows the island: that part applies.
        if (opcode === REMOVE) {
          let after = parent.nextSibling;
          while (after !== null && owned.has(after)) after = after.nextSibling;
          remove(root, parent.parentNode!, after);
        }
        break;
      }
      const index = u32[at + level]!;
      node =
        level === 0
          ? index < root.count - root.finals
            ? tailChild(root, index)
            : null
          : child(parent, index);
    }
    at += depth;
    if (inside || (node?.nodeType === 8 && opcode !== REMOVE)) {
      at += opcode === APPEND ? 2 : opcode === RETAG ? 1 : opcode === REMOVE ? 0 : 3;
      continue;
    }
    if (opcode === REMOVE) {
      remove(root, parent, node as ChildNode | null);
    } else if (opcode === APPEND) {
      let last = node as Node;
      while (last.nextSibling?.nodeType === 3 && !owned.has(last.nextSibling))
        last = last.nextSibling;
      const start = u32[at]!;
      const value =
        start >= SOURCE
          ? root.input.slice(start - SOURCE, u32[at + 1]!)
          : root.literal(start, u32[at + 1]!);
      last.parentNode!.insertBefore(document.createTextNode(value), last.nextSibling);
      at += 2;
    } else if (opcode === RETAG) {
      const element = node as Element;
      const tag = clone(root, u32[at]!, false) as Element;
      if (tag.tagName === element.tagName) {
        // As React updates an element: values in place, removed ones gone, new ones last.
        for (const { name } of Array.from(element.attributes))
          if (!tag.hasAttribute(name)) element.removeAttribute(name);
        for (const { name, value } of Array.from(tag.attributes))
          if (element.getAttribute(name) !== value) element.setAttribute(name, value);
      } else {
        while (element.firstChild) tag.appendChild(element.firstChild);
        element.replaceWith(tag);
      }
      at += 1;
    } else {
      const added = build(
        root,
        node as Element,
        words + u32[at]!,
        words + u32[at + 1]!,
        u32[at + 2]!,
      );
      if (depth === 0) root.count += added;
      at += 3;
    }
  }
}

/** Removes `next`, its later siblings, and the later siblings of each ancestor up to
 * the root. */
function remove(root: Root, parent: Node, next: ChildNode | null) {
  const { element } = root;
  for (;;) {
    // (A run of text nodes is one root child.)
    let text = false;
    while (next !== null) {
      const after: ChildNode | null = next.nextSibling;
      if (!owned.has(next)) {
        if (parent === element && !(text && next.nodeType === 3)) root.count -= 1;
        text = next.nodeType === 3;
      }
      parent.removeChild(next);
      next = after;
    }
    if (parent === element) break;
    next = (parent as ChildNode).nextSibling;
    parent = parent.parentNode!;
  }
}

/**
 * Builds the tokens `u32[start..end]` (their text from unit `text` of the rendering's)
 * at the end of `parent`: how many nodes it added to `parent` itself.
 */
function build(root: Root, parent: Element, start: number, end: number, text: number) {
  const { u32, tags, origins, input } = root;
  const document = parent.ownerDocument;
  let node: Node = parent;
  let added = 0;
  // A new element under `parent` is built detached and inserted once it is whole (one
  // insertion into the live tree, not one per node).
  let depth = 0;
  for (let at = start; at < end; at += 1) {
    const word = u32[at]!;
    const id = word >>> 2;
    const kind = word & 3;
    if (depth === 0 && kind !== CLOSE) added += 1;
    if (kind === OPEN && id >= INTERNED && root.interned[id - INTERNED]!.startsWith(ISLAND)) {
      // An island: a comment holds its place; its subtree is React's (see `islands`).
      const anchor = document.createComment("");
      (depth === 0 ? parent : node).appendChild(anchor);
      let level = 1;
      while (level > 0) {
        at += 1;
        const inner = u32[at]!;
        if ((inner & 3) === OPEN) level += 1;
        else if ((inner & 3) === CLOSE) level -= 1;
        else if ((inner & 3) === TEXT_KIND && u32[origins + at]! === NOWHERE) text += inner >>> 2;
      }
      continue;
    }
    if (kind === OPEN) {
      const element = clone(root, id, false);
      node = depth === 0 ? element : node.appendChild(element);
      depth += 1;
    } else if (kind === CLOSE) {
      depth -= 1;
      if (depth === 0) {
        parent.appendChild(node);
        node = parent;
      } else node = node.parentNode!;
    } else if (kind === VOID) node.appendChild(clone(root, id, true));
    else {
      const origin = u32[origins + at]!;
      if (origin !== NOWHERE) (node as Element).append(input.slice(origin, origin + id));
      else {
        (node as Element).append(root.literal(text, text + id));
        text += id;
      }
    }
  }
  // (An element left open at the end goes in as it is.)
  if (depth > 0) {
    while (node.parentNode !== null) node = node.parentNode;
    parent.appendChild(node);
  }
  return added;
}

/** A new node for tag `id`: an element (`OPEN`), or a whole subtree (`VOID`). */
function clone(root: Root, id: number, whole: boolean): Node {
  if (id < INTERNED) return root.tags[id]!.cloneNode(false);
  const at = id - INTERNED;
  let node = root.nodes[at];
  if (node === undefined) {
    // Parsed once: an open tag with its end tag, or the subtree as it is.
    const html = root.interned[at]!;
    const key = whole ? html : `${html}</${html.slice(1, html.search(/[\s>]/))}>`;
    node = root.parsed.get(key);
    if (node === undefined) {
      root.template.innerHTML = key;
      node = root.template.content.firstChild!;
      root.parsed.set(key, node);
    }
    root.nodes[at] = node;
  }
  return node.cloneNode(whole);
}
