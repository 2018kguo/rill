// Where the main thread waits during a run: idle gaps over 15 ms, with what ran just
// before and after (top-level tasks, their functions and URLs), and worker/network
// activity during the gap.
//   ENGINES=a node gaps.mjs "<query>" [throttle]
import { chromium } from "playwright";
import { CATEGORIES, serve, tracedRun } from "./trace-run.mjs";

const [query, throttle = "4"] = process.argv.slice(2);
CATEGORIES.push("disabled-by-default-devtools.timeline.stack");
const browser = await chromium.launch();
for (const name of (process.env.ENGINES ?? "rill").split(",")) {
  const { origin, server } = serve(new URL(`dist/${name}`, import.meta.url).pathname);
  const { events, stage } = await tracedRun(browser, origin, query, Number(throttle), "412x915@2.625");
  server.close();
  const main = events.find((e) => e.name === "thread_name" && e.args?.name === "CrRendererMain");
  const start = events.find((e) => e.name === "stage:start")?.ts;
  const end = start + (stage.stats.finished - stage.stats.started) * 1000;
  const tasks = events
    .filter((e) => e.pid === main.pid && e.tid === main.tid && e.name === "RunTask" && e.ph === "X" && e.ts + e.dur > start && e.ts < end)
    .sort((a, b) => a.ts - b.ts);
  const inside = (task) =>
    events
      .filter((e) => e.pid === main.pid && e.tid === main.tid && e.ts >= task.ts && e.ts < task.ts + task.dur && e.ph === "X" && ["FunctionCall", "EventDispatch", "TimerFire", "FireAnimationFrame", "v8.compile", "EvaluateScript", "ParseHTML", "ResourceReceivedData"].includes(e.name))
      .slice(0, 2)
      .map((e) => `${e.name}${e.args?.data?.functionName ? ":" + e.args.data.functionName + "L" + e.args.data.lineNumber + "C" + e.args.data.columnNumber : ""}${e.args?.data?.url ? "@" + e.args.data.url.split("/").pop() : ""}${e.args?.data?.type ? "(" + e.args.data.type + ")" : ""}`)
      .join(" ");
  console.log(`${name}: ${tasks.length} tasks; idle gaps > 15 ms:`);
  let total = 0;
  for (let index = 1; index < tasks.length; index += 1) {
    const before = tasks[index - 1];
    const gap = (tasks[index].ts - (before.ts + before.dur)) / 1000;
    if (gap < 15) continue;
    total += gap;
    const at = ((before.ts + before.dur - start) / 1000).toFixed(0);
    const net = events.filter((e) => e.name === "ResourceFinish" && e.ts >= before.ts + before.dur && e.ts <= tasks[index].ts).length;
    const worker = events.filter((e) => e.name === "RunTask" && e.tid !== main.tid && e.pid === main.pid && e.ts >= before.ts + before.dur && e.ts <= tasks[index].ts).reduce((s, e) => s + e.dur / 1000, 0);
    console.log(`  at ${at} ms gap ${gap.toFixed(0)} ms | before: ${inside(before)} | after: ${inside(tasks[index])} | net finishes ${net}, other-thread ms ${worker.toFixed(0)}`);
  }
  console.log(`  total ${total.toFixed(0)} ms`);
  if (process.env.TIMELINE) for (const task of tasks) if (task.dur > 1000) console.log(`   ${((task.ts - start) / 1000).toFixed(0).padStart(5)} +${(task.dur / 1000).toFixed(1).padStart(6)} ${inside(task)}`);
}
await browser.close();
