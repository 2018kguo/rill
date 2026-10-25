// CPU profile of one run (main thread), self time by function, top N.
//   node profile.mjs <engine dist name> "<query>" [throttle] [top]
import { chromium } from "playwright";
import { serve } from "./trace-run.mjs";
const [name, query, throttle = "4", top = "30"] = process.argv.slice(2);
const { origin, server } = serve(new URL(`dist/${name}`, import.meta.url).pathname);
const browser = await chromium.launch();
const context = await browser.newContext({ viewport: { width: 412, height: 915 }, deviceScaleFactor: 2.625 });
const page = await context.newPage();
await page.goto(`${origin}/stage.html?${query}`);
await page.waitForSelector("body[data-ready]", { state: "attached" });
await page.waitForLoadState("networkidle");
await page.waitForTimeout(800);
const cdp = await context.newCDPSession(page);
await cdp.send("Emulation.setCPUThrottlingRate", { rate: Number(throttle) });
await cdp.send("Profiler.enable");
await cdp.send("Profiler.setSamplingInterval", { interval: 100 });
await cdp.send("Profiler.start");
await page.evaluate(() => window.go());
await page.evaluate(() => new Promise((resolve) => (document.body.dataset.done ? resolve() : addEventListener("stage-done", resolve, { once: true }))));
const { profile } = await cdp.send("Profiler.stop");
await browser.close(); server.close();
const byId = new Map(profile.nodes.map((n) => [n.id, n]));
const self = new Map();
const dt = profile.timeDeltas;
profile.samples.forEach((id, i) => { const n = byId.get(id); const f = n.callFrame; const key = `${f.functionName || "(anon)"} ${f.url.split("/").pop()}:${f.lineNumber + 1}`; self.set(key, (self.get(key) ?? 0) + (dt[i] ?? 0) / 1000); });
const total = [...self.values()].reduce((a, b) => a + b, 0);
console.log(`total sampled ${total.toFixed(0)} ms`);
for (const [k, v] of [...self.entries()].sort((a, b) => b[1] - a[1]).slice(0, Number(top))) console.log(`${v.toFixed(1).padStart(7)} ms  ${k}`);
