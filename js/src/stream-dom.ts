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
const OPEN = 0;
const CLOSE = 1;
const VOID = 2;
/** Tag ids from here on: interned strings (an open tag, or a whole subtree as `VOID`). */
const INTERNED = 32;
/** An APPEND range in the text as given. */
const SOURCE = 0x80000000;
/** A text token that is not one stretch of the text as given. */
const NOWHERE = 0xffffffff;

export type StreamRenderer = {
  /** Shows `text`; `false` when it needs the exact path (the root is left as it was). */
  update(text: string): boolean;
  dispose(): void;
};

export function createStreamRenderer(root: Element, session: StreamSession, module: StreamModule): StreamRenderer {
  const { memory } = module;
  const document = root.ownerDocument;
  const template = document.createElement("template");
  const tags = module.tags.split("\n").map((tag) => {
    template.innerHTML = tag;
    return template.content.firstChild as Element;
  });
  const utf16 = new TextDecoder("utf-16le");
  const state: Root = {
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
      const u32 = state.u32;
      const words = u32[header + 3]!;
      // Only the text the edit refers to: from its first unit on.
      state.from = u32[header + 7]!;
      state.strings = undefined;
      state.input = next;
      state.origins = u32[header + 9]! - words;
      const interned = u32[header + 11]!;
      if (interned > state.interned.length) {
        const added = session.strings(state.interned.length).split("\0");
        for (let at = 0; at < added.length - 1; at += 1) state.interned.push(added[at]!);
      }
      if (outcome === PATCH) {
        apply(state, u32[header + 1]!, u32[header + 2]!, words);
      } else {
        root.textContent = "";
        state.finals = 0;
        state.from = 0;
        state.count = build(state, root, words, words + u32[header + 4]!, 0);
      }
      state.finals += u32[header + 8]!;
      return true;
    },
    dispose() {
      session.free();
    },
  };
}

/** The DOM side's state. */
type Root = {
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
  let node = parent.firstChild;
  for (; node !== null && index > 0; index -= 1) {
    if (node.nodeType === 3) while (node.nextSibling?.nodeType === 3) node = node.nextSibling;
    node = node.nextSibling;
  }
  return node;
}

/** The tail's root child `index` (from the end: the tail is the last block or two). */
function tailChild(root: Root, index: number): ChildNode | null {
  let node = root.element.lastChild;
  for (let steps = root.count - root.finals - 1 - index; steps > 0 && node !== null; steps -= 1) node = node.previousSibling;
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
    for (let level = 0; level < depth; level += 1) {
      parent = node!;
      const index = u32[at + level]!;
      node = level === 0 ? (index < root.count - root.finals ? tailChild(root, index) : null) : child(parent, index);
    }
    at += depth;
    if (opcode === REMOVE) {
      // The node, its later siblings, and the later siblings of each ancestor.
      let next = node as ChildNode | null;
      for (;;) {
        while (next !== null) {
          const after: ChildNode | null = next.nextSibling;
          parent.removeChild(next);
          if (parent === element) root.count -= 1;
          next = after;
        }
        if (parent === element) break;
        next = (parent as ChildNode).nextSibling;
        parent = parent.parentNode!;
      }
    } else if (opcode === APPEND) {
      let last = node as Node;
      while (last.nextSibling?.nodeType === 3) last = last.nextSibling;
      const start = u32[at]!;
      const value = start >= SOURCE ? root.input.slice(start - SOURCE, u32[at + 1]!) : root.literal(start, u32[at + 1]!);
      last.parentNode!.insertBefore(document.createTextNode(value), last.nextSibling);
      at += 2;
    } else {
      const added = build(root, node as Element, words + u32[at]!, words + u32[at + 1]!, u32[at + 2]!);
      if (depth === 0) root.count += added;
      at += 3;
    }
  }
}

/**
 * Builds the tokens `u32[start..end]` (their text from unit `text` of the rendering's)
 * at the end of `parent`: how many nodes it added to `parent` itself.
 */
function build(root: Root, parent: Element, start: number, end: number, text: number) {
  const { u32, tags, origins, input } = root;
  let node: Node = parent;
  let added = 0;
  for (let at = start; at < end; at += 1) {
    const word = u32[at]!;
    const id = word >>> 2;
    const kind = word & 3;
    if (node === parent && kind !== CLOSE) added += 1;
    if (kind === OPEN) node = node.appendChild(clone(root, id, false));
    else if (kind === CLOSE) node = node.parentNode!;
    else if (kind === VOID) node.appendChild(clone(root, id, true));
    else {
      const origin = u32[origins + at]!;
      if (origin !== NOWHERE) (node as Element).append(input.slice(origin, origin + id));
      else {
        (node as Element).append(root.literal(text, text + id));
        text += id;
      }
    }
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
    root.template.innerHTML = whole ? html : `${html}</${html.slice(1, html.search(/[\s>]/))}>`;
    node = root.nodes[at] = root.template.content.firstChild!;
  }
  return node.cloneNode(whole);
}
