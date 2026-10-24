// Browser benchmark for rill vs Streamdown, after js-framework-benchmark (durations from
// Chrome traces, warm-up, many runs, CPU throttling) and Tachometer (interleaved A/B runs,
// confidence intervals on the difference). Every number comes from a Chrome trace or the
// browser's own Long Animation Frames entries, never from JS timers in the page.
//
//   node bench.mjs --scenario hist-distinct [--scenario name=<stage query>] \
//     [--engine streamdown=dist/streamdown --engine rill=dist/rill] \
//     [--throttle 1,4,10] [--runs 15] [--warmups 2] [--screen 412x915@2.625] [--out results.json]
//
// Scenarios are named in scenarios.mjs (all four by default); the engines default to
// what `npm run build` built.
import { writeFileSync } from "node:fs";
import { chromium } from "playwright";
import * as Trace from "@paulirish/trace_engine/models/trace/trace.js";

import { ENGINES, SCENARIOS, scenario } from "./scenarios.mjs";
import { serve, tracedRun } from "./trace-run.mjs";

globalThis.DOMRect ??= class {
  constructor(x = 0, y = 0, width = 0, height = 0) {
    Object.assign(this, { x, y, width, height });
  }
};

const args = process.argv.slice(2);
const all = (name) => args.flatMap((arg, index) => (arg === name ? [args[index + 1]] : []));
const one = (name, fallback) => all(name)[0] ?? fallback;
const engines = all("--engine").length ? all("--engine").map((pair) => pair.split("=")) : ENGINES;
const scenarios = all("--scenario").length ? all("--scenario").map(scenario) : Object.keys(SCENARIOS).map(scenario);
const throttles = one("--throttle", "10").split(",").map(Number);
const runs = Number(one("--runs", "15"));
const warmups = Number(one("--warmups", "2"));
const out = one("--out", "results.json");
// A budget Android phone: 412×915 CSS pixels at 2.625× (e.g. Moto G-class).
const screen = one("--screen", "412x915@2.625");

const STYLE_LAYOUT = new Set(["UpdateLayoutTree", "Layout", "RecalculateStyles", "HitTest"]);
const PAINT = new Set(["PrePaint", "Paint", "Layerize", "Commit", "UpdateLayerTree", "UpdateLayer", "PaintImage"]);
const GC = /^(MinorGC|MajorGC|V8\.GC|BlinkGC)/;

/** Metrics for one traced run. Times in milliseconds. */
async function metrics(events, stage) {
  const model = Trace.TraceModel.Model.createWithAllHandlers();
  await model.parse(events);
  const data = model.parsedTrace().data;
  const marks = data.UserTimings.performanceMarks;
  const startMark = marks.find((mark) => mark.name === "stage:start");
  const start = startMark.ts;
  // A run ends when the DOM settled (highlighting included); the page records it.
  // It ends where js-framework-benchmark ends: once the last DOM change is painted, i.e.
  // at the end of the first main-thread paint commit after it.
  const settled = start + (stage.stats.finished - stage.stats.started) * 1000;
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
    // Off-main-thread work (rill highlights in a worker) is reported, not hidden.
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
  const loaf = stage.loaf.filter((entry) => entry.startTime >= stage.stats.started && entry.startTime <= stage.stats.finished);
  const wall = (end - start) / 1000;
  return {
    wallMs: wall,
    // History: every answer on screen. Stream: the last character handed to the engine.
    contentMs: (stage.stats.onScreen || stage.stats.revealed) - stage.stats.started,
    busyMs: busy,
    scriptMs: Math.max(0, busy - styleLayout - paint - gc),
    // The engine's own share: main-thread JavaScript and the garbage it leaves.
    scriptGcMs: Math.max(0, busy - styleLayout - paint),
    styleLayoutMs: styleLayout,
    paintMs: paint,
    gcMs: gc,
    idleMs: Math.max(0, wall - busy),
    workerMs: worker,
    frames: frames.length,
    droppedPct: frames.length ? (100 * dropped) / frames.length : 0,
    loafCount: loaf.length,
    loafBlockingMs: loaf.reduce((sum, entry) => sum + (entry.blockingDuration ?? 0), 0),
    loafMaxMs: loaf.reduce((max, entry) => Math.max(max, entry.duration), 0),
    heapMB: stage.heapBytes / 1e6,
    domNodes: stage.domNodes,
  };
}

const median = (values) => {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = sorted.length >> 1;
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
};
/** Seeded so a report reproduces. */
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

const servers = engines.map(([name, root]) => ({ name, ...serve(root) }));
const browser = await chromium.launch({ args: ["--disable-background-timer-throttling", "--disable-renderer-backgrounding"] });
const results = [];
for (const [scenario, query] of scenarios) {
  for (const throttle of throttles) {
    const samples = Object.fromEntries(engines.map(([name]) => [name, []]));
    for (let run = 0; run < warmups + runs; run += 1) {
      // Interleave, alternating which engine goes first.
      const order = run % 2 ? [...servers].reverse() : servers;
      for (const { name, origin } of order) {
        const { events, stage } = await tracedRun(browser, origin, query, throttle, screen);
        if (run >= warmups) samples[name].push(await metrics(events, stage));
      }
    }
    const summary = { scenario, throttle, screen, runs, engines: {} };
    for (const [name] of engines) {
      const list = samples[name];
      summary.engines[name] = Object.fromEntries(Object.keys(list[0]).map((key) => [key, list[0][key] === null ? null : median(list.map((sample) => sample[key]))]));
    }
    if (engines.length === 2) {
      const [a, b] = engines.map(([name]) => samples[name]);
      for (const key of ["wallMs", "busyMs", "scriptMs", "gcMs", "scriptGcMs", "loafBlockingMs", "heapMB"]) {
        summary[`${key}Ratio`] = { median: median(a.map((s) => s[key])) / median(b.map((s) => s[key])), ci95: ratioInterval(a.map((s) => s[key]), b.map((s) => s[key])) };
      }
    }
    summary.samples = samples;
    results.push(summary);
    const line = engines.map(([name]) => {
      const m = summary.engines[name];
      // The most any engine could gain here: its busy time over the browser's layout+paint floor.
      const ceiling = m.busyMs / (m.styleLayoutMs + m.paintMs);
      return `${name}: wall ${m.wallMs.toFixed(0)} ms (busy ${m.busyMs.toFixed(0)} = script ${m.scriptMs.toFixed(0)} + style/layout ${m.styleLayoutMs.toFixed(0)} + paint ${m.paintMs.toFixed(0)} + gc ${m.gcMs.toFixed(0)}; idle ${m.idleMs.toFixed(0)}; worker ${m.workerMs.toFixed(0)}), dropped ${m.droppedPct.toFixed(0)}% of ${m.frames} frames, LoAF blocking ${m.loafBlockingMs.toFixed(0)} ms, heap ${m.heapMB.toFixed(1)} MB, ${m.domNodes} DOM nodes, ceiling ${ceiling.toFixed(1)}×`;
    });
    console.log(`\n${scenario} @ ${throttle}× CPU, ${runs} runs (medians)\n  ${line.join("\n  ")}`);
    if (summary.wallMsRatio) {
      const r = summary.wallMsRatio;
      console.log(`  wall ratio ${r.median.toFixed(2)}× [95% CI ${r.ci95[0].toFixed(2)}–${r.ci95[1].toFixed(2)}], busy ratio ${summary.busyMsRatio.median.toFixed(2)}× [${summary.busyMsRatio.ci95.map((x) => x.toFixed(2)).join("–")}], script+gc ratio ${summary.scriptGcMsRatio.median.toFixed(2)}× [${summary.scriptGcMsRatio.ci95.map((x) => x.toFixed(2)).join("–")}]`);
    }
    writeFileSync(out, JSON.stringify(results, null, 1));
  }
}
await browser.close();
for (const { server } of servers) server.close();
