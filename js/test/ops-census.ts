// Which edit ops a 256-unit stream of a file produces (dev probe).
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
const { Stream, streamMemory: memory } = createRequire(import.meta.url)("../stream-wasm/rill_stream.js");
const source = readFileSync(process.argv[2]!, "utf8");
const s = new Stream();
const counts: Record<string, number> = {};
let html = 0;
let prev = 0;
const names = ["append", "remove", "insertHtml", "insertText"];
for (let end = 256; ; end += 256) {
  const cut = Math.min(end, source.length);
  const outcome = s.update(source.slice(prev, cut), prev === 0);
  prev = cut;
  s.text;
  const ops = new Uint32Array(memory().buffer, s.opsAt * 4, s.opsLength);
  let at = 0;
  const kinds: string[] = [];
  while (at < ops.length) {
    const op = ops[at]!; const depth = ops[at + 1]!; at += 2 + depth;
    kinds.push(names[op]!);
    if (op !== 1) { if (op === 2) html += ops[at + 1]! - ops[at]!; at += 2; }
  }
  const key = outcome === 0 ? kinds.join("+") || "(none)" : ["patch", "reset", "fallback"][outcome]!;
  counts[key] = (counts[key] ?? 0) + 1;
  if (cut === source.length) break;
}
console.log(Object.entries(counts).sort((a, b) => b[1] - a[1]).slice(0, 12), "html units inserted", html);
