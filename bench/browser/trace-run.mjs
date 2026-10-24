// One traced run of a stage scenario: returns raw trace events plus in-page stats.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join } from "node:path";
import { chromium } from "playwright";

const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".wasm": "application/wasm", ".woff2": "font/woff2", ".json": "application/json" };
export function serve(root) {
  const server = createServer(async (request, response) => {
    const path = join(root, decodeURIComponent(new URL(request.url, "http://x").pathname));
    let body;
    try {
      body = await readFile(path);
    } catch {
      response.writeHead(404);
      response.end();
      return;
    }
    response.writeHead(200, { "content-type": types[extname(path)] ?? "application/octet-stream" });
    response.end(body);
  }).listen(0);
  return { server, origin: `http://localhost:${server.address().port}` };
}

export const CATEGORIES = [
  "devtools.timeline",
  "disabled-by-default-devtools.timeline",
  "disabled-by-default-devtools.timeline.frame",
  "blink.user_timing",
  "toplevel",
  "v8.execute",
  "loading",
  "latencyInfo",
];

/** Runs one scenario in a fresh context of `browser`; resolves to { events, stage }. */
/** `screen` is "<width>x<height>@<device scale factor>" (default: a laptop window). */
export async function tracedRun(browser, origin, query, throttle, screen = "1000x900@2") {
  const [, width, height, scale] = screen.match(/^(\d+)x(\d+)@([\d.]+)$/);
  const context = await browser.newContext({ viewport: { width: Number(width), height: Number(height) }, deviceScaleFactor: Number(scale) });
  const page = await context.newPage();
  await page.route(/^(?!http:\/\/localhost)/, (route) => route.abort());
  await page.goto(`${origin}/stage.html?${query}`);
  await page.waitForSelector("body[data-ready]", { state: "attached" });
  await page.waitForLoadState("networkidle");
  await page.waitForTimeout(800);
  const cdp = await context.newCDPSession(page);
  if (throttle > 1) await cdp.send("Emulation.setCPUThrottlingRate", { rate: throttle });
  const events = [];
  cdp.on("Tracing.dataCollected", ({ value }) => events.push(...value));
  const complete = new Promise((resolve) => cdp.once("Tracing.tracingComplete", resolve));
  await cdp.send("Tracing.start", { traceConfig: { includedCategories: CATEGORIES, recordMode: "recordAsMuchAsPossible" }, transferMode: "ReportEvents" });
  await page.evaluate(() => window.go());
  // One call that resolves on the page's own event: Playwright's selector polling ran on
  // the page's main thread and was counted against both engines.
  await page.evaluate(() => new Promise((resolve) => (document.body.dataset.done ? resolve() : addEventListener("stage-done", resolve, { once: true }))));
  await page.waitForTimeout(500);
  await cdp.send("Tracing.end");
  await complete;
  const stage = await page.evaluate(() => window.stage);
  // Memory still held once the scene is done: live JS heap after a full GC, DOM nodes.
  await cdp.send("HeapProfiler.collectGarbage");
  stage.heapBytes = (await cdp.send("Runtime.getHeapUsage")).usedSize;
  // Nodes in the rendered answer (detached nodes and page chrome are not output).
  stage.domNodes = await page.evaluate(() => {
    const walker = document.createTreeWalker(document.getElementById("answer"));
    let count = 0;
    while (walker.nextNode()) count += 1;
    return count;
  });
  await context.close();
  return { events, stage };
}
