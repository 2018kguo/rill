// What a streaming update costs, counted instead of timed: the drop-in <Streamdown> streams
// each document family at n, 2n, 4n and 8n characters in a DOM (happy-dom, React's client
// renderer, so memo and refs live across updates as in a browser), and every UTF-16 unit
// handed to the WebAssembly (markup, definitions, splitsAt) or lexed by marked (the block
// split) during its last updates is counted. That work must follow the block being
// written, not the whole answer: it may not grow with the answer, except in families
// whose block being written is the answer (a table, a code block, a list, a text with
// footnotes), where it may grow linearly but no faster.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { mount } from "./dom.ts";

const { Lexer } = await import("marked");
const { initRill } = await import("../src/index.ts");
const { Streamdown } = await import("../src/streamdown.ts");

const rill = await initRill(readFileSync(new URL("../wasm/rill_bg.wasm", import.meta.url)));

/** Units handed to each piece of work since the last reset. */
const work = { markup: 0, definitions: 0, splitsAt: 0, lexed: 0 };
const total = () => work.markup + work.definitions + work.splitsAt + work.lexed;
const count = <F extends (source: string, ...rest: never[]) => unknown>(
  name: keyof typeof work,
  run: F
): F =>
  ((source: string, ...rest: never[]) => {
    work[name] += source.length;
    return run(source, ...rest);
  }) as F;
rill.markup = count("markup", rill.markup!);
rill.piece = count("markup", rill.piece!);
rill.footnotes = count("definitions", rill.footnotes!);
rill.definitions = count("definitions", rill.definitions!);
rill.splitsAt = count("splitsAt", rill.splitsAt!);
const blockTokens = Lexer.prototype.blockTokens;
Lexer.prototype.blockTokens = function (this: InstanceType<typeof Lexer>, source: string, ...rest: never[]) {
  work.lexed += source.length;
  return (blockTokens as (...args: unknown[]) => unknown).call(this, source, ...rest);
} as typeof blockTokens;

const CHUNK = 24;
const TIMED = 20;

/** Streams `text` into a fresh <Streamdown>; the units its last `TIMED` updates cost. */
async function lastUpdates(text: string): Promise<number> {
  const view = mount(Streamdown);
  const ends: number[] = [];
  for (let end = CHUNK; end < text.length; end += CHUNK) ends.push(end);
  ends.push(text.length);
  let counted = 0;
  for (const [index, end] of ends.entries()) {
    if (index === ends.length - TIMED) counted = total();
    await view.render(text.slice(0, end), { isAnimating: true });
  }
  await view.unmount();
  return total() - counted;
}

const PROSE =
  "Paragraph with **bold**, *emphasis*, `code`, a [link](https://example.com/a_b) and " +
  "a_snake_case word.\n\n- item with ~~strike~~\n- item two\n  1. nested\n\n> a quote " +
  "with $5 and 3 > 2\n\n";
const fill = (unit: string, n: number) => unit.repeat(Math.ceil(n / unit.length));
const numbered = (n: number, line: (index: number) => string) => {
  let out = "";
  for (let index = 0; out.length < n; index += 1) out += line(index);
  return out;
};

/** Families whose block being written stays small however long the answer grows. */
const bounded: Record<string, (n: number) => string> = {
  prose: (n) => fill(PROSE, n),
  "code blocks": (n) => numbered(n, (index) => `Text ${index}.\n\n\`\`\`ts\nconst x${index} = ${index};\n\`\`\`\n\n`),
  headings: (n) => numbered(n, (index) => `## Heading ${index}\n\nSome *text* ${index}.\n\n`),
  "an open bracket": (n) =>
    `[an aside that never closes\n\n${fill("A paragraph with no brackets at all, *emphasis* and `code`.\n\n", n)}`,
  "reference links": (n) =>
    `[spec]: https://example.com/spec\n[1]: https://www.rfc-editor.org/rfc/rfc9114\n\n${fill("See [the spec][spec] and [RFC 9114][1] for details, plus [spec].\n\n", n)}`,
  // Footnotes number across the answer, which Streamdown keeps in one block: rill renders
  // it as pieces too, each after the calls before it.
  "prose after footnotes": (n) =>
    `Rust checks ownership[^own] at compile time[^zero].\n\n[^own]: The binding that holds a value.\n[^zero]: No cost at run time.\n\n${fill("More prose citing the owner[^own], with *emphasis* and `code`.\n\n", n)}`,
  // Definitions that keep arriving: each new one may change any earlier reference.
  "definitions streaming": (n) =>
    `${numbered(n / 2, (index) => `See [r${index}] here.\n\n`)}${numbered(n / 2, (index) => `[r${index}]: https://example.com/${index}\n\n`)}`,
};

/** Families whose block being written is the whole answer. */
const growing: Record<string, (n: number) => string> = {
  table: (n) => `| a | b | c |\n|---|:-:|--:|\n${numbered(n, (index) => `| ${index} | *x* | \`y\` |\n`)}`,
  "code block": (n) => `\`\`\`rust\n${numbered(n, (index) => `let x${index} = ${index};\n`)}\`\`\`\n`,
  list: (n) => numbered(n, (index) => `- item ${index} with *a*\n`),
  "long paragraph": (n) => numbered(n, (index) => `line ${index} with *a* and \`b\`\n`),
  blockquote: (n) => numbered(n, (index) => `> line ${index} with *a*\n`),
  // Footnote definitions being written: the footnote section (one element after the
  // answer) holds them all, and renders whole on each update.
  "footnote definitions": (n) => `${numbered(n / 2, (index) => `Text[^${index}].\n\n`)}${numbered(n / 2, (index) => `[^${index}]: Note ${index}.\n\n`)}`,
};

const SIZES = [2000, 4000, 8000, 16000];

async function measure(build: (n: number) => string) {
  const units: number[] = [];
  for (const size of SIZES) units.push(await lastUpdates(build(size)));
  return units;
}

const report = (units: number[]) => SIZES.map((size, index) => `${size}: ${units[index]}`).join(", ");

for (const [name, build] of Object.entries(bounded)) {
  test(`streaming ${name}: an update's work does not grow with the answer`, async () => {
    const units = await measure(build);
    assert.ok(units.at(-1)! <= 1.5 * units[0]! + 2000, `work grows with the answer (${report(units)})`);
  });
}

for (const [name, build] of Object.entries(growing)) {
  test(`streaming a ${name}: an update's work grows at most linearly`, async () => {
    const units = await measure(build);
    for (let index = 1; index < units.length; index += 1) {
      assert.ok(units[index]! <= 2.5 * units[index - 1]! + 2000, `faster than linear (${report(units)})`);
    }
  });
}
