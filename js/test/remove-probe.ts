// Dev probe: what the old and new renderings hold where an update removes nodes.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
const { Stream, streamMemory: memory } = createRequire(import.meta.url)("../stream-wasm/rill_stream.js");
const source = readFileSync(process.argv[2]!, "utf8");
const s = new Stream();
let prev = 0, shown = 0;
let lastHtml = "";
for (let end = 256; end < 256 * 40; end += 256) {
  const cut = Math.min(end, source.length);
  s.update(source.slice(prev, cut), prev === 0);
  prev = cut;
  const h = new Uint32Array(memory().buffer, s.headerAt() * 4, 9);
  const ops = Array.from(new Uint32Array(memory().buffer, h[1]! * 4, h[2]));
  const html = s.html;
  if (ops[0] === 1 || ops.includes(1)) {
    if (shown++ < 6) console.log("OLD END:", JSON.stringify(lastHtml.slice(-90)), "\nNEW AT:", JSON.stringify(html.slice(lastHtml.length - 90, lastHtml.length + 30)), "\nops", ops.slice(0, 12).join(","), "\n");
  }
  lastHtml = html;
}
