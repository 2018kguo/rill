// Opcode decoder for rill's WebAssembly output, mirroring `hast::Encoder` in
// crates/rill/src/hast.rs. Text that borrows the source is sliced from `source` (NUL
// replaced, as Rust saw it).
const OP_ROOT = 0;
const OP_ELEMENT = 1;
const OP_TEXT = 2;
const OP_RAW = 3;
const OP_NEWLINE = 4;
const VALUE_STRING = 0;
const VALUE_TRUE = 1;
const VALUE_FALSE = 2;
const VALUE_NUMBER = 3;

type Point = { line: number; column: number; offset: number };
export type HastNode = {
  type: string;
  tagName?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
  value?: string;
  data?: unknown;
  position?: { start?: Point; end?: Point };
};

export function decodeHast(
  ops: Uint32Array,
  table: string,
  source: string
): HastNode {
  return new HastReader(ops, table, source).node();
}

/** Reads rill's opcodes from `at`: a HAST node, or (for the drop-in component, which
 * embeds HAST in its own program) the strings and numbers around one. */
export class HastReader {
  at = 0;
  readonly ops: Uint32Array;
  readonly table: string;
  readonly source: string;

  constructor(ops: Uint32Array, table: string, source: string) {
    this.ops = ops;
    this.table = table;
    this.source = source;
  }

  next(): number {
    const op = this.ops[this.at];
    if (op === undefined) {
      throw new Error(`rill HAST ended early at opcode ${this.at}`);
    }
    this.at += 1;
    return op;
  }

  string(): string {
    const from = this.next() === 0 ? this.source : this.table;
    const start = this.next();
    return from.slice(start, this.next());
  }

  node(): HastNode {
    const next = () => this.next();
    const readString = () => this.string();
    const readPoint = (): Point => ({
      line: next(),
      column: next(),
      offset: next(),
    });
    const readPosition = (target: HastNode) => {
      const flags = next();
      if (flags === 3) {
        target.position = { start: readPoint(), end: readPoint() };
      } else if (flags === 1) {
        target.position = { start: readPoint() };
      } else if (flags === 2) {
        target.position = { end: readPoint() };
      }
    };
    const readChildren = () => {
      const count = next();
      const nodes = new Array<HastNode>(count);
      for (let index = 0; index < count; index += 1) {
        nodes[index] = readNode();
      }
      return nodes;
    };
    const readValue = (): unknown => {
      const kind = next();
      if (kind === VALUE_STRING) {
        return readString();
      }
      if (kind === VALUE_TRUE) {
        return true;
      }
      if (kind === VALUE_FALSE) {
        return false;
      }
      if (kind === VALUE_NUMBER) {
        return next();
      }
      const count = next();
      const list = new Array<string>(count);
      for (let index = 0; index < count; index += 1) {
        list[index] = readString();
      }
      return list;
    };
    function readNode(): HastNode {
      const op = next();
      if (op === OP_NEWLINE) {
        return { type: "text", value: "\n" };
      }
      if (op === OP_TEXT || op === OP_RAW) {
        const leaf: HastNode = {
          type: op === OP_TEXT ? "text" : "raw",
          value: readString(),
        };
        readPosition(leaf);
        return leaf;
      }
      if (op === OP_ELEMENT) {
        const tagName = readString();
        const properties: Record<string, unknown> = {};
        for (let count = next(); count > 0; count -= 1) {
          const name = readString();
          properties[name] = readValue();
        }
        const meta = next() === 1 ? readString() : undefined;
        const element: HastNode = {
          type: "element",
          tagName,
          properties,
          children: readChildren(),
        };
        if (meta !== undefined) {
          element.data = { meta };
        }
        readPosition(element);
        return element;
      }
      if (op === OP_ROOT) {
        const quirks = next() === 1;
        const root: HastNode = { type: "root", children: readChildren() };
        if (quirks) {
          root.data = { quirksMode: false };
        }
        readPosition(root);
        return root;
      }
      throw new Error(`Unknown rill HAST opcode ${op}`);
    }
    return readNode();
  }
}
