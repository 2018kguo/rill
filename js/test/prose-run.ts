// Dev probe: streams a file through the DOM renderer in 256-unit chunks, checks the end.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import "./dom.ts";
const { createStreamRenderer } = await import("../src/stream-dom.ts");
const { Stream, streamMemory, streamTags, renderStream } = createRequire(import.meta.url)("../stream-wasm/rill_stream.js");
const source = readFileSync(process.argv[2]!, "utf8");
const root = document.createElement("div");
const r = createStreamRenderer(root, new Stream(), { memory: streamMemory(), tags: streamTags() });
for (let end = 256; ; end += 256) {
  const cut = Math.min(end, source.length);
  if (!r.update(source.slice(0, cut))) console.log("fallback at", cut);
  if (cut === source.length) break;
}
root.normalize();
const want = renderStream(source);
console.log(root.innerHTML === want ? "same" : "DIFFERENT", root.innerHTML.length, want?.length);
