// Runs perf.html in Chromium at a CPU throttle and prints each engine's times.
//
//   node demo/perf/perf.mjs [--build] [--throttle 10] [--runs 3] [--scenes history,table,code]
//        [--engines streamdown,rill,streamdown-block,rill-block]
//
// `--build` bundles perf.tsx with production React first.
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { dirname, extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const here = dirname(fileURLToPath(import.meta.url));
const dist = join(here, "dist");
const args = process.argv.slice(2);
const option = (name, fallback) => {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : fallback;
};
const throttle = Number(option("--throttle", "10"));
const runs = Number(option("--runs", "3"));
const scenes = option("--scenes", "history,table,code").split(",");
const engines = option("--engines", "streamdown,rill").split(",");
const step = option("--step", "24");
const profile = args.includes("--profile");

if (args.includes("--build") || !existsSync(join(dist, "perf.js"))) {
  // The page, and the highlighting worker beside it (js/src/highlight.ts loads it
  // from next to the bundle).
  for (const entry of [join(here, "perf.tsx"), join(here, "../../js/src/highlight-worker.ts")]) build(entry);
}

function build(entry) {
  const result = spawnSync(
    "bun",
    ["build", entry, "--outdir", dist, "--target", "browser", "--splitting",
      "--define", 'process.env.NODE_ENV="production"'],
    { encoding: "utf8" }
  );
  if (result.status !== 0) {
    console.error(result.stdout, result.stderr);
    process.exit(1);
  }
  copyFileSync(join(here, "perf.html"), join(dist, "perf.html"));
  copyFileSync(join(here, "../../js/wasm/rill_bg.wasm"), join(dist, "rill_bg.wasm"));
}

const types = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm" };
const server = createServer(async (request, response) => {
  const path = join(dist, decodeURIComponent(new URL(request.url, "http://x").pathname));
  try {
    const body = await readFile(path);
    response.writeHead(200, { "content-type": types[extname(path)] ?? "application/octet-stream" });
    response.end(body);
  } catch {
    response.writeHead(404);
    response.end();
  }
}).listen(0);

/** Prints where the main thread spent its samples: self and inclusive time. */
function report(engine, profile) {
  const byId = new Map(profile.nodes.map((node) => [node.id, node]));
  const parent = new Map();
  for (const node of profile.nodes) for (const child of node.children ?? []) parent.set(child, node.id);
  // Only the samples before the content appeared (perf.tsx's contentMarker), if found.
  let end = profile.samples.length;
  for (let index = 0; index < profile.samples.length; index += 1) {
    if (byId.get(profile.samples[index]).callFrame.functionName === "contentMarker") {
      end = index;
      break;
    }
  }
  if (end < profile.samples.length) console.log(`${engine}: until the content appeared`);
  const counts = new Map();
  for (const id of profile.samples.slice(0, end)) counts.set(id, (counts.get(id) ?? 0) + 1);
  const interval = (profile.endTime - profile.startTime) / profile.samples.length / 1000;
  const self = new Map();
  const inclusive = new Map();
  let busy = 0;
  for (const [id, count] of counts) {
    const frame = byId.get(id).callFrame;
    if (frame.functionName === "(idle)") continue;
    busy += count;
    const key = `${frame.functionName || "(anonymous)"} ${frame.url.split("/").pop()}:${frame.lineNumber}`;
    self.set(key, (self.get(key) ?? 0) + count);
    const seen = new Set();
    for (let cursor = id; cursor !== undefined; cursor = parent.get(cursor)) {
      const caller = byId.get(cursor).callFrame;
      const name = `${caller.functionName || "(anonymous)"} ${caller.url.split("/").pop()}:${caller.lineNumber}`;
      if (seen.has(name)) continue;
      seen.add(name);
      inclusive.set(name, (inclusive.get(name) ?? 0) + count);
    }
  }
  const ms = (count) => `${(count * interval).toFixed(0).padStart(6)} ms`;
  console.log(`${engine}: busy ${ms(busy)}`);
  console.log("  self:");
  for (const [key, count] of [...self].sort((a, b) => b[1] - a[1]).slice(0, 18)) console.log(`  ${ms(count)} ${key}`);
  console.log("  inclusive:");
  for (const [key, count] of [...inclusive].sort((a, b) => b[1] - a[1]).slice(0, 30)) console.log(`  ${ms(count)} ${key}`);
}

const browser = await chromium.launch();
const median = (values) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];
for (const scene of scenes) {
  const results = {};
  for (let run = 0; run < runs; run += 1) {
    for (const engine of engines) {
      const page = await browser.newPage({ viewport: { width: 960, height: 1080 } });
      await page.route(/^(?!http:\/\/localhost)/, (route) => route.abort());
      page.on("pageerror", (error) => console.error("pageerror:", error.message));
      await page.goto(`http://localhost:${server.address().port}/perf.html?engine=${engine}&scene=${scene}&step=${step}${profile ? "&marker" : ""}`);
      await page.waitForSelector("body[data-ready]", { state: "attached" });
      const cdp = await page.context().newCDPSession(page);
      await cdp.send("Emulation.setCPUThrottlingRate", { rate: throttle });
      await page.evaluate(() => new Promise((resolve) => setTimeout(resolve, 300)));
      if (profile) {
        await cdp.send("Profiler.enable");
        await cdp.send("Profiler.setSamplingInterval", { interval: 200 });
        await cdp.send("Profiler.start");
      }
      await cdp.send("Performance.enable");
      const taskTime = async () =>
        (await cdp.send("Performance.getMetrics")).metrics.find((metric) => metric.name === "TaskDuration").value;
      const before = await taskTime();
      await page.evaluate(() => window.perf.start());
      // Polled rarely: Playwright's own polling would run on the measured main thread.
      await page.waitForFunction(() => document.body.dataset.done === "true", undefined, {
        polling: 1000,
        timeout: 600_000,
      });
      // Main-thread time (throttled), less the 600 ms quiet wait's idle frames.
      const busy = (await taskTime()) - before;
      if (profile) report(engine, (await cdp.send("Profiler.stop")).profile);
      const stats = { ...(await page.evaluate(() => window.perf.stats)), busy: busy * 1000 };
      (results[engine] ??= []).push(stats);
      await page.close();
    }
  }
  console.log(`${scene} (${throttle}x CPU throttle, median of ${runs}):`);
  for (const engine of engines) {
    const list = results[engine];
    const content = median(list.map((stats) => stats.content));
    const settled = median(list.map((stats) => stats.settled));
    const busy = median(list.map((stats) => stats.busy));
    const updates = list[0].updates ? `, ${list[0].updates} updates` : "";
    console.log(`  ${engine.padEnd(18)} content on screen ${(content / 1000).toFixed(2)} s, settled ${(settled / 1000).toFixed(2)} s, main thread busy ${(busy / 1000).toFixed(2)} s${updates}`);
  }
}
await browser.close();
server.close();
