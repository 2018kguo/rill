// What a page loads, per engine: the stage's files fetched until it is ready (a history
// chat with code), gzipped. With `--by package` (or `--by file`), the JavaScript's
// gzip-weighted bytes by the source each came from, side by side, largest differences
// first (needs `SOURCEMAP=1 bun build.ts ...`).
//
//   node bytes.mjs [--by package|file] [engine ...]   (default: streamdown rill)
import { existsSync, readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { eachMapping, TraceMap } from "@jridgewell/trace-mapping";
import { chromium } from "playwright";
import { serve } from "./trace-run.mjs";

const args = process.argv.slice(2);
const by = args.includes("--by") ? args[args.indexOf("--by") + 1] : null;
const engines = args.filter((arg, index) => !arg.startsWith("--") && args[index - 1] !== "--by");
if (engines.length === 0) engines.push("streamdown", "rill");

const sourceOf = (source) => {
  const inPackage = source.match(/node_modules\/((?:@[^/]+\/)?[^/]+)\/(.*)/);
  if (inPackage) return by === "file" ? `${inPackage[1]}/${inPackage[2]}` : inPackage[1];
  const own = source.match(/\/(js\/src|js\/wasm|stage)\/(.*)/);
  if (own) return by === "file" ? `${own[1]}/${own[2]}` : own[1];
  return source;
};

const attributed = {};
for (const engine of engines) {
  const root = new URL(`dist/${engine}`, import.meta.url).pathname;
  const { origin, server } = serve(root);
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const fetched = new Set();
  page.on("response", (response) => fetched.add(new URL(response.url()).pathname));
  await page.goto(`${origin}/stage.html?scene=history&answers=lru-cache&overlay=0`);
  await page.waitForSelector("body[data-ready]", { state: "attached" });
  await page.waitForLoadState("networkidle");
  await browser.close();
  server.close();
  const totals = {};
  const sources = (attributed[engine] = {});
  for (const path of fetched) {
    if (!existsSync(root + path)) continue;
    const bytes = readFileSync(root + path);
    const kind = path.split(".").pop();
    totals[kind] ??= { files: 0, gzip: 0 };
    totals[kind].files += 1;
    totals[kind].gzip += gzipSync(bytes, { level: 9 }).length;
    if (!(by && kind === "js" && existsSync(`${root + path}.map`))) continue;
    const code = bytes.toString("utf8");
    const lines = code.split("\n");
    const ratio = gzipSync(bytes, { level: 9 }).length / bytes.length;
    const segments = [];
    eachMapping(new TraceMap(readFileSync(`${root + path}.map`, "utf8")), (segment) => segments.push(segment));
    segments.sort((a, b) => a.generatedLine - b.generatedLine || a.generatedColumn - b.generatedColumn);
    for (const [index, segment] of segments.entries()) {
      const next = segments[index + 1];
      const end = next?.generatedLine === segment.generatedLine ? next.generatedColumn : lines[segment.generatedLine - 1].length;
      const key = segment.source ? sourceOf(segment.source) : "(unmapped)";
      sources[key] = (sources[key] ?? 0) + (end - segment.generatedColumn) * ratio;
    }
  }
  console.log(engine.padEnd(16), Object.entries(totals).map(([kind, total]) => `${kind} ${(total.gzip / 1024).toFixed(1)} KB (${total.files})`).join(", "));
}
if (by) {
  const keys = new Set(engines.flatMap((engine) => Object.keys(attributed[engine])));
  const rows = [...keys].map((key) => [key, ...engines.map((engine) => attributed[engine][key] ?? 0)]);
  const spread = (row) => Math.max(...row.slice(1)) - Math.min(...row.slice(1));
  rows.sort((a, b) => spread(b) - spread(a));
  console.log(`\n${"source".padEnd(48)}${engines.map((engine) => engine.padStart(14)).join("")}`);
  for (const row of rows.filter((row) => spread(row) > 300)) {
    console.log(`${row[0].slice(0, 48).padEnd(48)}${row.slice(1).map((value) => `${(value / 1024).toFixed(1)} KB`.padStart(14)).join("")}`);
  }
}
