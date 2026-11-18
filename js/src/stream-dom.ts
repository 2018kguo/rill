// The DOM half of rill-stream: keeps a root element's children equal to Streamdown's
// blocks for a streaming text. Final blocks stay as they are; each update applies the
// session's edit to the open tail (usually one appendData).

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
      finals += session.finalNodes;
      return true;
    },
    dispose() {
      session.free();
    },
  };
}

/** Runs an edit program against the tail of `root` (from child `finals` on). */
function apply(root: Element, finals: number, ops: Uint32Array, strings: string) {
  let at = 0;
  while (at < ops.length) {
    const opcode = ops[at]!;
    const depth = ops[at + 1]!;
    at += 2;
    // The node at the path (depth 0: the root itself); a REMOVE's path may end one past
    // its parent's last child.
    let node: Node | undefined = root;
    const chain: Node[] = [];
    for (let level = 0; level < depth; level += 1) {
      const index = ops[at + level]! + (level === 0 ? finals : 0);
      chain.push(node!);
      node = node!.childNodes[index];
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
    if (opcode === APPEND) (node as Text).appendData(value);
    else if (opcode === INSERT) (node as Element).insertAdjacentHTML("beforeend", value);
    else (node as Element).append(value);
  }
}
