// Per frame, how much style and layout work each engine causes: one traced run per engine,
// summing UpdateLayoutTree's elementCount, Layout's dirty objects and Paint's area.
//   node restyle.mjs "<query>" [throttle]
import { chromium } from "playwright";
import { CATEGORIES, serve, tracedRun } from "./trace-run.mjs";
import { ENGINES } from "./scenarios.mjs";

const [query, throttle = "4"] = process.argv.slice(2);
CATEGORIES.push("disabled-by-default-devtools.timeline.invalidationTracking");
const browser = await chromium.launch();
const pair = process.env.ENGINES ? process.env.ENGINES.split(",").map((name) => [name, new URL(`dist/${name}`, import.meta.url).pathname]) : ENGINES;
for (const [name, root] of pair) {
  const { origin, server } = serve(root);
  const { events } = await tracedRun(browser, origin, query, Number(throttle), "412x915@2.625");
  server.close();
  const main = events.find((e) => e.name === "thread_name" && e.args?.name === "CrRendererMain");
  const on = events.filter((e) => e.pid === main.pid && e.tid === main.tid);
  const sum = (name, f) => on.filter((e) => e.name === name).reduce((s, e) => s + (f(e) ?? 0), 0);
  const count = (name) => on.filter((e) => e.name === name && e.ph === "X").length;
  const styles = on.filter((e) => e.name === "UpdateLayoutTree" && e.ph === "X");
  const elements = styles.map((e) => e.args?.elementCount ?? 0).sort((a, b) => a - b);
  const layouts = on.filter((e) => e.name === "Layout" && e.ph === "X");
  const dirty = layouts.map((e) => e.args?.beginData?.dirtyObjects ?? 0).sort((a, b) => a - b);
  const reasons = {};
  for (const e of on.filter((e) => e.name === "StyleInvalidatorInvalidationTracking" || e.name === "StyleRecalcInvalidationTracking")) {
    const key = `${e.name.replace("InvalidationTracking", "")}:${e.args?.data?.reason ?? e.args?.data?.invalidationList?.[0]?.classes ?? "?"}`;
    reasons[key] = (reasons[key] ?? 0) + 1;
  }
  const pct = (list, p) => list[Math.floor(list.length * p)] ?? 0;
  console.log(`${name}: style recalcs ${styles.length}, elements total ${elements.reduce((a, b) => a + b, 0)} (p50 ${pct(elements, 0.5)}, p95 ${pct(elements, 0.95)}, max ${elements.at(-1)}), ms ${(sum("UpdateLayoutTree", (e) => e.dur) / 1000).toFixed(0)}`);
  console.log(`  layouts ${layouts.length}, dirty objects total ${dirty.reduce((a, b) => a + b, 0)} (p50 ${pct(dirty, 0.5)}, p95 ${pct(dirty, 0.95)}), ms ${(sum("Layout", (e) => e.dur) / 1000).toFixed(0)}; paints ${count("Paint")} ms ${(sum("Paint", (e) => e.dur) / 1000).toFixed(0)}`);
  console.log("  invalidations", Object.entries(reasons).sort((a, b) => b[1] - a[1]).slice(0, 8).map(([k, v]) => `${k} ${v}`).join("; "));
}
await browser.close();
