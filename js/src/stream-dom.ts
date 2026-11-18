// The DOM half of rill-stream: keeps a root element's children equal to Streamdown's
// blocks for a streaming text. Final blocks stay as they are; each update applies the
// session's edit to the open tail.
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
  /** Where the update's header is, as a `u32` index into memory: the outcome, the
   * opcodes' (crates/rill-stream/src/diff.rs) `u32` index and count, the strings' byte
   * index and length (UTF-8), the root nodes that became final, the new tags' string
   * ranges' `u32` index and count. */
  headerAt(): number;
  /** After a reset: the whole HTML. */
  readonly html: string;
  free(): void;
};

const PATCH = 0;
const FALLBACK = 2;
const APPEND = 0;
const REMOVE = 1;
const BUILD = 2;
const OPEN = 0;
const CLOSE = 1;
const VOID = 2;

export type StreamRenderer = {
  /** Shows `text`; `false` when it needs the exact path (the root is left as it was). */
  update(text: string): boolean;
  dispose(): void;
};

/** `memory`: the module's (its `streamMemory()` export). */
export function createStreamRenderer(root: Element, session: StreamSession, memory: WebAssembly.Memory): StreamRenderer {
  let text = "";
  /** Root children before the open tail. */
  let finals = 0;
  let input = new Uint16Array(0);
  let header = new Uint32Array(0);
  const utf8 = new TextDecoder();
  /** An element of each tag the session has built, by its id. */
  const tags: Element[] = [];
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
      if (header.buffer !== memory.buffer) header = new Uint32Array(memory.buffer, session.headerAt() * 4, 8);
      if (outcome === PATCH) {
        const buffer = memory.buffer;
        const strings = utf8.decode(new Uint8Array(buffer, header[3], header[4]));
        if (header[7] !== 0) define(root.ownerDocument, tags, new Uint32Array(buffer, header[6]! * 4, header[7]), strings);
        apply(root, finals, new Uint32Array(buffer, header[1]! * 4, header[2]), strings, tags);
      } else {
        root.innerHTML = session.html;
        finals = 0;
      }
      finals += header[5]!;
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
function apply(root: Element, finals: number, ops: Uint32Array, strings: string, tags: Element[]) {
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
    if (opcode === BUILD) {
      at = build(node as Element, ops, at, strings, tags);
      continue;
    }
    const value = strings.slice(ops[at]!, ops[at + 1]!);
    at += 2;
    if (opcode === APPEND) {
      let last = node as Node;
      while (last.nextSibling?.nodeType === 3) last = last.nextSibling;
      last.parentNode!.insertBefore(document.createTextNode(value), last.nextSibling);
    } else (node as Element).append(value);
  }
}

/** Parses each new tag (string ranges in `defs`) into an element to clone. */
function define(document: Document, tags: Element[], defs: Uint32Array, strings: string) {
  const template = document.createElement("template");
  for (let at = 0; at < defs.length; at += 2) {
    template.innerHTML = strings.slice(defs[at]!, defs[at + 1]!);
    tags.push(template.content.firstChild as Element);
  }
}

/**
 * Builds a `BUILD` op's items (from `ops[at]`, their count) at the end of `parent`:
 * elements are clones of parsed tags, several times cheaper than the HTML parser for
 * the short fragments a stream inserts. Returns where the next op starts.
 */
function build(parent: Element, ops: Uint32Array, at: number, strings: string, tags: Element[]) {
  let node: Node = parent;
  for (let count = ops[at++]!; count > 0; count -= 1) {
    const word = ops[at++]!;
    switch (word & 3) {
      case OPEN:
        node = node.appendChild(tags[word >>> 2]!.cloneNode(false));
        break;
      case CLOSE:
        node = node.parentNode!;
        break;
      case VOID:
        node.appendChild(tags[word >>> 2]!.cloneNode(false));
        break;
      default:
        (node as Element).append(strings.slice(ops[at]!, ops[at + 1]!));
        at += 2;
    }
  }
  return at;
}
