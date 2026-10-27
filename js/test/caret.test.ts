// `caretView` must give Streamdown's root the same two answers as the whole block: its own
// open-fence and delimiter-row tests, taken from the installed dist, run on the view and
// on the text, at every step of streams over short documents (every one over the
// characters that matter), the benchmark's answers, and texts that shrink or switch.
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { caretView } from "../src/caret.ts";

/** Streamdown's own tests, from its dist (they are not exported). */
function streamdownTests(): { fence: (block: string) => boolean; delimiter: (block: string) => boolean } {
  const dist = dirname(fileURLToPath(import.meta.resolve("streamdown")));
  for (const file of readdirSync(dist).filter((name) => name.endsWith(".js"))) {
    const source = readFileSync(join(dist, file), "utf8");
    const found = source.match(
      /(\w+)=(\/\^\[ \\t\]\{0,3\}\(`\{3,\}\|~\{3,\}\)\/),(\w+)=(\/[^\n]*?\/),(\w+)=(e=>\{let t=e\.split\(`\n`\)[\s\S]*?return n!==null\}),(\w+)=(e=>\{let t=e\.split\(`\n`\);[\s\S]*?return {1,2}false\})/
    );
    if (!found) continue;
    const [, fenceName, fence, rowName, row, , fenceTest, , rowTest] = found;
    // biome-ignore lint/security/noGlobalEval: Streamdown's own code, from its package
    const make = new Function(`const ${fenceName}=${fence},${rowName}=${row};return [${fenceTest},${rowTest}];`);
    const [fenceFn, rowFn] = make() as [(block: string) => boolean, (block: string) => boolean];
    return { fence: fenceFn, delimiter: rowFn };
  }
  throw new Error("Streamdown's caret tests not found in its dist: check caret.ts against the new version");
}

const { fence, delimiter } = streamdownTests();

const check = (text: string) => {
  const view = caretView(text);
  assert.equal(String(view), text);
  assert.equal(fence(view), fence(text), JSON.stringify(text));
  assert.equal(delimiter(view), delimiter(text), JSON.stringify(text));
};

test("every short document, streamed a unit at a time and cut back", () => {
  const pieces = ["```", "~~~", "`", "|", "-", ":", "\n", " ", "a", "\r"];
  const documents = (length: number): string[] =>
    length === 0 ? [""] : documents(length - 1).flatMap((head) => pieces.map((piece) => head + piece));
  for (const document of documents(5)) {
    for (let end = 0; end <= document.length; end += 1) check(document.slice(0, end));
    // Cut back to every length (the change lands mid-line), then grown again.
    for (let end = document.length - 1; end >= 0; end -= 1) {
      check(document.slice(0, end));
      check(document);
    }
  }
});

test("the benchmark's answers, streamed, then cut back and switched", () => {
  const stage = new URL("../../bench/browser/stage/", import.meta.url);
  const answers = readdirSync(stage)
    .filter((name) => name.endsWith(".md"))
    .map((name) => readFileSync(new URL(name, stage), "utf8"));
  assert.ok(answers.length > 3);
  for (const answer of answers) {
    for (let end = 0; end <= answer.length; end += 37) check(answer.slice(0, end));
    check(answer);
    // A text that shrinks (an edit) and grows again, and another message between.
    for (let end = answer.length; end > 0; end = Math.floor(end / 2)) check(answer.slice(0, end));
    check(answer);
    check(answers[0]!);
    check(answer);
  }
});

test("other splits go to the text", () => {
  const view = caretView("a\nb|c\n```\n");
  assert.deepEqual(view.split("|"), ["a\nb", "c\n```\n"]);
  assert.deepEqual(view.split("\n", 1), ["a"]);
  assert.deepEqual(view.split("\n"), ["```"]);
});
