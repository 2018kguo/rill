// The DOM half of rill-stream: keeps a root element's children equal to Streamdown's
// blocks for a streaming text. Final blocks stay as they are; each update applies the
// session's edit to the open tail.
//
// The module renders a token stream (crates/rill-stream/src/tokens.rs): a `u32` per
// element open, close or text run, the text in UTF-16. Elements are clones of one
// parsed element per tag; the edit (src/diff.rs) points into the tokens in place.
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
   * became final. */
  headerAt(): number;
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
/** Tag ids from here on: an ordered list starting at `id - OL_START`. */
const OL_START = 16;
const OL = 11;

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
  let text = "";
  /** Root children before the open tail. */
  let finals = 0;
  let input = new Uint16Array(0);
  let header = new Uint32Array(0);
  /** Writes `next[from..]` into the module's input buffer: its length. */
  const write = (next: string, from: number) => {
    const length = next.length - from;
    // Memory growth detaches the view; the buffer itself stays where it is.
    if (input.length < length || input.buffer !== memory.buffer) {
      const capacity = Math.max(length, input.length, 4096);
      input = new Uint16Array(memory.buffer, session.inputAt(capacity) * 2, capacity);
    }
    for (let at = 0; at < length; at += 1) input[at] = next.charCodeAt(from + at);
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
      const buffer = memory.buffer;
      if (header.buffer !== buffer) header = new Uint32Array(buffer, session.headerAt() * 4, 9);
      const words = new Uint32Array(buffer, header[3]! * 4, header[4]);
      // Only the text the edit refers to: from its first unit on.
      const from = header[7]!;
      const strings = utf16.decode(new Uint16Array(buffer, header[5]! * 2 + from * 2, header[6]! - from));
      if (outcome === PATCH) {
        apply(root, finals, new Uint32Array(buffer, header[1]! * 4, header[2]), words, strings, from, tags);
      } else {
        root.textContent = "";
        finals = 0;
        build(root, words, 0, words.length, strings, 0, tags);
      }
      finals += header[8]!;
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

/**
 * Runs an edit program against the tail of `root` (from child `finals` on). `strings` is
 * the rendering's text from unit `from` on.
 */
function apply(
  root: Element,
  finals: number,
  ops: Uint32Array,
  words: Uint32Array,
  strings: string,
  from: number,
  tags: Element[],
) {
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
      let next = (node ?? null) as ChildNode | null;
      while (level > 0) {
        const parent = chain[level - 1]!;
        while (next !== null) {
          const after: ChildNode | null = next.nextSibling;
          parent.removeChild(next);
          next = after;
        }
        level -= 1;
        next = level > 0 ? (chain[level]!.nextSibling as ChildNode | null) : null;
      }
    } else if (opcode === APPEND) {
      let last = node as Node;
      while (last.nextSibling?.nodeType === 3) last = last.nextSibling;
      const value = strings.slice(ops[at]! - from, ops[at + 1]! - from);
      last.parentNode!.insertBefore(document.createTextNode(value), last.nextSibling);
      at += 2;
    } else {
      build(node as Element, words, ops[at]!, ops[at + 1]!, strings, ops[at + 2]! - from, tags);
      at += 3;
    }
  }
}

/** Builds the tokens `[start, end)` (their text from `strings[text]`) at the end of `parent`. */
function build(parent: Element, words: Uint32Array, start: number, end: number, strings: string, text: number, tags: Element[]) {
  let node: Node = parent;
  for (let at = start; at < end; at += 1) {
    const word = words[at]!;
    const id = word >>> 2;
    switch (word & 3) {
      case OPEN:
        node = node.appendChild(clone(tags, id));
        break;
      case CLOSE:
        node = node.parentNode!;
        break;
      case VOID:
        node.appendChild(clone(tags, id));
        break;
      default:
        (node as Element).append(strings.slice(text, text + id));
        text += id;
    }
  }
}

function clone(tags: Element[], id: number): Node {
  if (id < OL_START) return tags[id]!.cloneNode(false);
  const list = tags[OL]!.cloneNode(false) as Element;
  list.setAttribute("start", String(id - OL_START));
  return list;
}
