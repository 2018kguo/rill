// Prints Streamdown's DOM and rill-stream's for each JSON-string argument (dev probe).
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { mount } from "./dom.ts";
const { Streamdown } = await import("streamdown");
const { initRill } = await import("../src/index.ts");
const rill = await initRill(readFileSync(new URL("../wasm/rill_bg.wasm", import.meta.url)));
const { renderStream } = createRequire(import.meta.url)("../stream-wasm/rill_stream.js");
const sd = mount(Streamdown);
const scratch = document.createElement("div");
const strip = (html: string) => html.replace(/ class="[^"]*"/g, "").replace(/ data-streamdown="[^"]*"/g, "");
for (const arg of process.argv.slice(2)) {
  const text = JSON.parse(arg);
  const healed = rill.remend!(text) ?? text;
  const want = (await sd.render(text, { controls: false })).split("\n[")[0]!;
  const html = renderStream(healed);
  scratch.innerHTML = `<div>${html ?? "FALLBACK"}</div>`;
  console.log(`${JSON.stringify(text)} healed ${JSON.stringify(healed)}\n  sd:   ${strip(want).replace(/^<div>/, "")}\n  ours: ${strip(scratch.innerHTML).replace(/^<div>/, "")}`);
}
await sd.unmount();
