// Memory after one run of a scenario, per engine, after a full GC: the main thread's JS
// heap, the WebAssembly linear memory there (which only grows), and the DOM's nodes and
// listeners (with how many are detached: the stage's own mutation log holds every node it
// saw change, so streams hold some). Not the highlighting worker's: Playwright opens no
// DevTools session on a worker.
//
//   node memory.mjs "<query>" [engine ...]   (default: streamdown rill)
import { chromium } from "playwright";
import { serve } from "./trace-run.mjs";

const [query, ...given] = process.argv.slice(2);
const engines = given.length ? given : ["streamdown", "rill"];
const MB = (bytes) => (bytes / 1048576).toFixed(1);
// Every WebAssembly.Memory the page creates (rill's and rill-highlight's), for its size.
const captureMemories = () => {
  const memories = (globalThis.__memories = []);
  const note = (result) => {
    const instance = result.instance ?? result;
    for (const value of Object.values(instance.exports ?? {})) if (value instanceof WebAssembly.Memory) memories.push(value);
    return result;
  };
  for (const name of ["instantiate", "instantiateStreaming"]) {
    const original = WebAssembly[name];
    WebAssembly[name] = (...args) => original.apply(WebAssembly, args).then(note);
  }
  const Instance = WebAssembly.Instance;
  WebAssembly.Instance = function (...args) { return note(new Instance(...args)); };
  WebAssembly.Instance.prototype = Instance.prototype;
};
for (const engine of engines) {
  const { origin, server } = serve(new URL(`dist/${engine}`, import.meta.url).pathname);
  const browser = await chromium.launch();
  const context = await browser.newContext({ viewport: { width: 412, height: 915 }, deviceScaleFactor: 2.625 });
  await context.addInitScript(captureMemories);
  const page = await context.newPage();
  const cdp = await context.newCDPSession(page);
  await page.goto(`${origin}/stage.html?${query}`);
  await page.waitForSelector("body[data-ready]", { state: "attached" });
  await page.evaluate(() => window.go());
  await page.evaluate(() => new Promise((r) => (document.body.dataset.done ? r() : addEventListener("stage-done", r, { once: true }))));
  await page.waitForTimeout(500);
  await cdp.send("HeapProfiler.collectGarbage");
  const { usedSize } = await cdp.send("Runtime.getHeapUsage");
  const wasm = await page.evaluate(() => globalThis.__memories.reduce((sum, memory) => sum + memory.buffer.byteLength, 0));
  const { nodes, jsEventListeners } = await cdp.send("Memory.getDOMCounters");
  const attached = await page.evaluate(() => {
    let count = 1;
    const walker = document.createTreeWalker(document, NodeFilter.SHOW_ALL);
    while (walker.nextNode()) count += 1;
    return count;
  });
  console.log(`${engine.padEnd(11)} main heap ${MB(usedSize)} MB, wasm ${MB(wasm)} MB, DOM ${nodes} nodes (${attached} in the document, ${nodes - attached} detached), ${jsEventListeners} listeners`);
  await browser.close();
  server.close();
}
