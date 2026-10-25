// Streamdown vs rill in Vercel's ai-chatbot template, production builds, traced in
// Chromium the way bench/browser is (durations from Chrome traces, interleaved runs,
// bootstrap intervals). See README.md for the setup (two builds of the template, one
// per engine, serving the same database).
//
//   node bench.mjs --app streamdown=http://localhost:4801 --app rill=http://localhost:4802 \
//     [--scenario open|stream] [--throttle 4,10] [--runs 5] [--warmups 1] \
//     [--screen 412x915@2.625] [--out results.json]
//
// Scenarios:
//   open    Open the seeded chat of 18 long answers (seed.mjs): navigation to settled.
//   stream  In a new chat, send a prompt; the template's mock model streams answer.md
//           (8 KB: five code blocks, three tables, a 30-item list) at 2,400
//           characters a second. Enter to settled.
// "Settled": the scenario's content is all there, the page's DOM has not changed for
// 600 ms with no frame gap over 100 ms, and every code line is highlighted. A run ends
// at the end of the first paint commit after the last DOM change.
import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";
import { chromium } from "playwright";
import * as Trace from "@paulirish/trace_engine/models/trace/trace.js";

import { CHAT_ID } from "./seed.mjs";

globalThis.DOMRect ??= class {
  constructor(x = 0, y = 0, width = 0, height = 0) {
    Object.assign(this, { x, y, width, height });
  }
};

const args = process.argv.slice(2);
const all = (name) => args.flatMap((arg, index) => (arg === name ? [args[index + 1]] : []));
const one = (name, fallback) => all(name)[0] ?? fallback;
const apps = all("--app").map((pair) => pair.split(/=(.*)/s).slice(0, 2));
if (apps.length === 0) throw new Error("--app name=url, once per build");
const scenarios = all("--scenario").length ? all("--scenario") : ["open", "stream"];
const throttles = one("--throttle", "4,10").split(",").map(Number);
const runs = Number(one("--runs", "5"));
const warmups = Number(one("--warmups", "1"));
const out = one("--out", "results.json");
const screen = one("--screen", "412x915@2.625");
const OPEN_ANSWERS = 18;
const PROMPT = "Write the benchmark answer";

const CATEGORIES = [
  "devtools.timeline",
  "disabled-by-default-devtools.timeline",
  "disabled-by-default-devtools.timeline.frame",
  "blink.user_timing",
  "toplevel",
  "v8.execute",
  "loading",
  "latencyInfo",
];

// In every page, before the app's scripts: Long Animation Frames, and `__bench.settle`,
// the done rule (as bench/browser's stage applies it to its answer, here to the page).
function instrument() {
  const loaf = [];
  try {
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) {
        loaf.push({ startTime: entry.startTime, duration: entry.duration, blockingDuration: entry.blockingDuration });
      }
    }).observe({ type: "long-animation-frame", buffered: true });
  } catch {}
  const stats = { started: 0, finished: 0, dataAt: 0 };
  const raw = () => document.querySelector('[data-streamdown="code-block-body"] span[style*="--sdm-c: inherit"]');
  const ready = {
    open: (count) => document.querySelectorAll('[data-testid="message-assistant"]').length >= count,
    stream: () => {
      const answers = document.querySelectorAll('[data-testid="message-assistant"]');
      return answers.length > 0 && answers[answers.length - 1].textContent.includes("End of answer.");
    },
  };
  window.__bench = {
    stats,
    loaf,
    settle(scenario, count) {
      stats.started = performance.now();
      performance.mark("stage:start");
      let lastMutation = performance.now();
      let quietSince = lastMutation;
      let lastTick = lastMutation;
      new MutationObserver(() => (lastMutation = performance.now())).observe(document, {
        subtree: true,
        childList: true,
        attributes: true,
        characterData: true,
      });
      return new Promise((resolve) => {
        const tick = () => {
          const now = performance.now();
          if (now - lastTick > 100) quietSince = now;
          lastTick = now;
          if (document.body && ready[scenario](count) && now - Math.max(lastMutation, quietSince) > 600 && !raw()) {
            stats.finished = lastMutation;
            const data = scenario === "open" && performance.getEntriesByType("resource").find((entry) => entry.name.includes("/api/messages"));
            stats.dataAt = data ? data.responseEnd : 0;
            resolve();
            return;
          }
          requestAnimationFrame(tick);
        };
        requestAnimationFrame(tick);
      });
    },
  };
  // The open scenario's clock starts with the document.
  if (location.pathname.startsWith("/chat/") && sessionStorage.getItem("bench:open")) {
    sessionStorage.removeItem("bench:open");
    window.__bench.done = window.__bench.settle("open", Number(sessionStorage.getItem("bench:count")));
  }
}

const STYLE_LAYOUT = new Set(["UpdateLayoutTree", "Layout", "RecalculateStyles", "HitTest"]);
const PAINT = new Set(["PrePaint", "Paint", "Layerize", "Commit", "UpdateLayerTree", "UpdateLayer", "PaintImage"]);
const GC = /^(MinorGC|MajorGC|V8\.GC|BlinkGC)/;

async function metrics(events, page) {
  const model = Trace.TraceModel.Model.createWithAllHandlers();
  await model.parse(events);
  const data = model.parsedTrace().data;
  const startMark = data.UserTimings.performanceMarks.filter((mark) => mark.name === "stage:start").at(-1);
  const start = startMark.ts;
  const settled = start + (page.stats.finished - page.stats.started) * 1000;
  const pid = startMark.pid;
  const mainTid = events.find((event) => event.ph === "M" && event.name === "thread_name" && event.pid === pid && event.args?.name === "CrRendererMain")?.tid;
  const commit = events
    .filter((event) => event.pid === pid && event.tid === mainTid && event.ph === "X" && event.name === "Commit" && event.ts >= settled)
    .reduce((first, event) => (!first || event.ts < first.ts ? event : first), null);
  const end = commit ? commit.ts + commit.dur : settled;
  const clip = (event) => Math.max(0, Math.min(event.ts + (event.dur ?? 0), end) - Math.max(event.ts, start)) / 1000;
  const workerTids = new Set(events.filter((event) => event.ph === "M" && event.name === "thread_name" && event.pid === pid && /Worker/.test(event.args?.name ?? "")).map((event) => event.tid));
  let worker = 0;
  let busy = 0;
  let styleLayout = 0;
  let paint = 0;
  let gc = 0;
  for (const event of events) {
    if (event.pid !== pid || event.ph !== "X") continue;
    if (workerTids.has(event.tid)) {
      if (event.name === "RunTask") worker += clip(event);
      continue;
    }
    if (event.tid !== mainTid) continue;
    if (event.name === "RunTask") busy += clip(event);
    else if (STYLE_LAYOUT.has(event.name)) styleLayout += clip(event);
    else if (PAINT.has(event.name)) paint += clip(event);
    else if (GC.test(event.name)) gc += clip(event);
  }
  const frames = data.Frames.frames.filter((frame) => frame.startTime >= start && frame.startTime < end && !frame.idle);
  const dropped = frames.filter((frame) => frame.dropped).length;
  const loaf = page.loaf.filter((entry) => entry.startTime >= page.stats.started && entry.startTime <= page.stats.finished);
  const wall = (end - start) / 1000;
  return {
    wallMs: wall,
    // open: from the messages' response to settled (the rest is the app loading).
    afterDataMs: page.stats.dataAt ? wall - (page.stats.dataAt - page.stats.started) : null,
    busyMs: busy,
    scriptMs: Math.max(0, busy - styleLayout - paint - gc),
    scriptGcMs: Math.max(0, busy - styleLayout - paint),
    styleLayoutMs: styleLayout,
    paintMs: paint,
    gcMs: gc,
    workerMs: worker,
    frames: frames.length,
    droppedPct: frames.length ? (100 * dropped) / frames.length : 0,
    loafBlockingMs: loaf.reduce((sum, entry) => sum + (entry.blockingDuration ?? 0), 0),
    heapMB: page.heapBytes / 1e6,
    domNodes: page.domNodes,
  };
}

/** The settled answers' HTML (the Streamdown roots), for the identity check. */
const answersHtml = () =>
  [...document.querySelectorAll('[data-testid="message-assistant"] [data-testid="message-content"]')]
    .map((element) => element.innerHTML)
    .join("\n<!-- next answer -->\n");

async function run(browser, origin, scenario, throttle) {
  const [, width, height, scale] = screen.match(/^(\d+)x(\d+)@([\d.]+)$/);
  const context = await browser.newContext({ viewport: { width: Number(width), height: Number(height) }, deviceScaleFactor: Number(scale) });
  await context.addInitScript(instrument);
  const page = await context.newPage();
  await page.route(/^(?!http:\/\/localhost)/, (route) => route.abort());
  const problems = [];
  page.on("pageerror", (error) => problems.push(error.message));
  // A guest session, and the app's code in the HTTP cache (a returning visitor).
  await page.goto(`${origin}/`);
  await page.waitForLoadState("networkidle");
  await page.waitForTimeout(800);
  const cdp = await context.newCDPSession(page);
  if (throttle > 1) await cdp.send("Emulation.setCPUThrottlingRate", { rate: throttle });
  const events = [];
  cdp.on("Tracing.dataCollected", ({ value }) => events.push(...value));
  const complete = new Promise((resolve) => cdp.once("Tracing.tracingComplete", resolve));
  if (scenario === "stream") await page.locator('[data-testid="multimodal-input"]').fill(PROMPT);
  await cdp.send("Tracing.start", { traceConfig: { includedCategories: CATEGORIES, recordMode: "recordAsMuchAsPossible" }, transferMode: "ReportEvents" });
  if (scenario === "open") {
    await page.evaluate((count) => {
      sessionStorage.setItem("bench:open", "1");
      sessionStorage.setItem("bench:count", String(count));
    }, OPEN_ANSWERS);
    await page.goto(`${origin}/chat/${CHAT_ID}`);
    await page.evaluate(() => window.__bench.done);
  } else {
    const done = page.evaluate(() => {
      window.__bench.done = window.__bench.settle("stream", 1);
    });
    await done;
    await page.keyboard.press("Enter");
    await page.evaluate(() => window.__bench.done);
  }
  await page.waitForTimeout(500);
  await cdp.send("Tracing.end");
  await complete;
  const stats = await page.evaluate(() => ({ stats: window.__bench.stats, loaf: window.__bench.loaf }));
  await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 });
  await cdp.send("HeapProfiler.collectGarbage");
  stats.heapBytes = (await cdp.send("Runtime.getHeapUsage")).usedSize;
  stats.domNodes = await page.evaluate(() => document.getElementsByTagName("*").length);
  const html = await page.evaluate(answersHtml);
  await context.close();
  if (problems.length) console.warn(`  page errors (${origin}): ${[...new Set(problems)].join("; ")}`);
  return { events, page: stats, html };
}

const median = (values) => {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = sorted.length >> 1;
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
};
function random(seed) {
  let state = seed >>> 0 || 1;
  return () => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    return (state >>> 0) / 4294967296;
  };
}
/** 95% bootstrap interval of the ratio of medians a/b. */
function ratioInterval(a, b, draws = 2000) {
  const next = random(7);
  const sample = (values) => values.map(() => values[Math.floor(next() * values.length)]);
  const ratios = [];
  for (let draw = 0; draw < draws; draw += 1) ratios.push(median(sample(a)) / median(sample(b)));
  ratios.sort((x, y) => x - y);
  return [ratios[Math.floor(draws * 0.025)], ratios[Math.floor(draws * 0.975)]];
}

const browser = await chromium.launch({ args: ["--disable-background-timer-throttling", "--disable-renderer-backgrounding"] });
const results = [];
const hashes = {};
for (const scenario of scenarios) {
  for (const throttle of throttles) {
    const samples = Object.fromEntries(apps.map(([name]) => [name, []]));
    for (let index = 0; index < warmups + runs; index += 1) {
      const order = index % 2 ? [...apps].reverse() : apps;
      for (const [name, origin] of order) {
        const { events, page, html } = await run(browser, origin, scenario, throttle);
        const hash = createHash("sha1").update(html).digest("hex").slice(0, 12);
        (hashes[`${scenario}@${throttle} ${name}`] ??= new Set()).add(hash);
        if (index >= warmups) samples[name].push({ ...(await metrics(events, page)), hash });
      }
    }
    const summary = { scenario, throttle, screen, runs, apps: {} };
    for (const [name] of apps) {
      const list = samples[name];
      summary.apps[name] = Object.fromEntries(
        Object.keys(list[0])
          .filter((key) => key !== "hash")
          .map((key) => [key, list[0][key] === null ? null : median(list.map((sample) => sample[key]))])
      );
    }
    if (apps.length === 2) {
      const [a, b] = apps.map(([name]) => samples[name]);
      for (const key of ["wallMs", "afterDataMs", "busyMs", "scriptGcMs", "loafBlockingMs", "heapMB"]) {
        if (a[0][key] === null) continue;
        summary[`${key}Ratio`] = { median: median(a.map((s) => s[key])) / median(b.map((s) => s[key])), ci95: ratioInterval(a.map((s) => s[key]), b.map((s) => s[key])) };
      }
    }
    summary.samples = samples;
    results.push(summary);
    console.log(`\n${scenario} @ ${throttle}× CPU, ${runs} runs (medians)`);
    for (const [name] of apps) {
      const m = summary.apps[name];
      console.log(
        `  ${name}: wall ${m.wallMs.toFixed(0)} ms${m.afterDataMs === null ? "" : ` (after data ${m.afterDataMs.toFixed(0)})`}, busy ${m.busyMs.toFixed(0)} = script+gc ${m.scriptGcMs.toFixed(0)} + style/layout ${m.styleLayoutMs.toFixed(0)} + paint ${m.paintMs.toFixed(0)}; worker ${m.workerMs.toFixed(0)}; dropped ${m.droppedPct.toFixed(0)}% of ${m.frames} frames; LoAF blocking ${m.loafBlockingMs.toFixed(0)} ms; heap ${m.heapMB.toFixed(1)} MB; ${m.domNodes} elements`
      );
    }
    for (const [key, value] of Object.entries(summary).filter(([key]) => key.endsWith("Ratio"))) {
      console.log(`  ${key.replace("Ratio", "")} ${apps[0][0]}/${apps[1][0]}: ${value.median.toFixed(2)}× [95% CI ${value.ci95.map((x) => x.toFixed(2)).join("–")}]`);
    }
    writeFileSync(out, JSON.stringify({ results, hashes: Object.fromEntries(Object.entries(hashes).map(([k, v]) => [k, [...v]])) }, null, 1));
  }
}
await browser.close();
console.log("\nFinal answer HTML per scenario and app (sha1 prefix):");
for (const [key, set] of Object.entries(hashes)) console.log(`  ${key}: ${[...set].join(", ")}`);
