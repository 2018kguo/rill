// Per-update cost while an answer streams must follow the block being written, not the
// whole answer: an update to an answer of 2n characters costs what one to n characters
// costs. Each document family streams at n, 2n, 4n and 8n, and the last updates of each
// are timed through what <Streamdown> runs per update (remend, the block split, the
// pieces of a reference-link block, the markup of the block that changed).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { parseMarkdownIntoBlocks, splitDefinitionBlock } from "../src/blocks.ts";
import { initRill } from "../src/index.ts";

const rill = await initRill(readFileSync(new URL("../wasm/rill_bg.wasm", import.meta.url)));

const options = { prefix: "", linkSafety: true, ssr: false, overrides: "", spine: true };
const CHUNK = 24;
const TIMED = 40;

const PROSE =
  "Paragraph with **bold**, *emphasis*, `code`, a [link](https://example.com/a_b) and " +
  "a_snake_case word.\n\n- item with ~~strike~~\n- item two\n  1. nested\n\n> a quote " +
  "with $5 and 3 > 2\n\n";
const CODE = "Some text before.\n\n```ts\nconst map = new Map<string, number>();\n" +
  "for (const [key, value] of map) console.log(key, value * 2);\n```\n\n";
const REFERENCES = "See [the spec][spec] and [RFC 9114][1] for details, plus [spec].\n\n";

/** One update as the drop-in runs it: heal, split, render what changed. */
function update(text: string): void {
  const healed = rill.remend!(text)!;
  const blocks = parseMarkdownIntoBlocks(healed);
  const pieces = blocks.length === 1 ? (splitDefinitionBlock(blocks[0]!) ?? blocks) : blocks;
  rill.markup!(pieces.at(-1)!, {}, options);
}

/** Streams `text`; the time of its last `TIMED` updates, best of a few streams. */
function lastUpdates(text: string): number {
  let best = Number.POSITIVE_INFINITY;
  for (let round = 0; round < 3; round += 1) {
    const ends: number[] = [];
    for (let end = CHUNK; end < text.length; end += CHUNK) ends.push(end);
    ends.push(text.length);
    const timedFrom = ends.length - TIMED;
    let start = 0;
    for (const [index, end] of ends.entries()) {
      if (index === timedFrom) start = performance.now();
      update(text.slice(0, end));
    }
    best = Math.min(best, performance.now() - start);
  }
  return best;
}

const families: Record<string, (n: number) => string> = {
  prose: (n) => PROSE.repeat(Math.ceil(n / PROSE.length)),
  code: (n) => CODE.repeat(Math.ceil(n / CODE.length)),
  // Definitions make the answer one block in Streamdown; rill renders its pieces.
  "reference links": (n) =>
    `[spec]: https://example.com/spec\n[1]: https://www.rfc-editor.org/rfc/rfc9114\n\n${REFERENCES.repeat(Math.ceil(n / REFERENCES.length))}`,
};

for (const [name, build] of Object.entries(families)) {
  test(`streaming ${name}: per-update cost does not grow with the answer`, () => {
    const sizes = [4096, 8192, 16384, 32768];
    const times = sizes.map((size) => lastUpdates(build(size)));
    const report = sizes.map((size, index) => `${size}: ${times[index]!.toFixed(2)} ms`).join(", ");
    for (let index = 1; index < sizes.length; index += 1) {
      assert.ok(times[index]! <= 2.2 * times[index - 1]!, `grows faster than 2.2x per doubling (${report})`);
    }
    // Linear per-update work would make 8n cost 8x; the block being written does not grow.
    assert.ok(times.at(-1)! <= 4 * times[0]!, `per-update cost follows the answer (${report})`);
  });
}
