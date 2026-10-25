// After one run, a heap snapshot's biggest holders: self size by node type and name, and
// the largest strings with what retains them.
//   node heap.mjs <engine dist name> "<query>"
import { chromium } from "playwright";
import { serve } from "./trace-run.mjs";

const [name, query] = process.argv.slice(2);
const { origin, server } = serve(new URL(`dist/${name}`, import.meta.url).pathname);
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 412, height: 915 } });
await page.goto(`${origin}/stage.html?${query}`);
await page.waitForSelector("body[data-ready]", { state: "attached" });
await page.waitForTimeout(800);
await page.evaluate(() => window.go());
await page.evaluate(() => new Promise((resolve) => (document.body.dataset.done ? resolve() : addEventListener("stage-done", resolve, { once: true }))));
const cdp = await page.context().newCDPSession(page);
await cdp.send("HeapProfiler.enable");
await cdp.send("HeapProfiler.collectGarbage");
let chunks = "";
cdp.on("HeapProfiler.addHeapSnapshotChunk", ({ chunk }) => (chunks += chunk));
await cdp.send("HeapProfiler.takeHeapSnapshot", { reportProgress: false });
await browser.close();
server.close();
const snap = JSON.parse(chunks);
const meta = snap.snapshot.meta;
const nf = meta.node_fields, nl = nf.length, types = meta.node_types[0];
const ef = meta.edge_fields, el = ef.length, etypes = meta.edge_types[0];
const nodes = snap.nodes, edges = snap.edges, strings = snap.strings;
const T = nf.indexOf("type"), N = nf.indexOf("name"), S = nf.indexOf("self_size"), E = nf.indexOf("edge_count");
const ET = ef.indexOf("type"), EN = ef.indexOf("name_or_index"), EO = ef.indexOf("to_node");
const by = new Map();
let total = 0;
for (let i = 0; i < nodes.length; i += nl) {
  const type = types[nodes[i + T]];
  const key = type === "string" || type === "concatenated string" || type === "sliced string" ? type : `${type}:${strings[nodes[i + N]]}`;
  by.set(key, (by.get(key) ?? 0) + nodes[i + S]);
  total += nodes[i + S];
}
console.log(`total ${(total / 1e6).toFixed(1)} MB`);
for (const [k, v] of [...by].sort((a, b) => b[1] - a[1]).slice(0, 15)) console.log(`${(v / 1e6).toFixed(2).padStart(8)} MB  ${k.slice(0, 90)}`);
// Who points at the largest objects: one level of retainers for the top 8 by self size.
const firstEdge = new Uint32Array(nodes.length / nl + 1);
for (let i = 0, e = 0; i < nodes.length; i += nl) { firstEdge[i / nl] = e; e += nodes[i + E] * el; }
const retainers = new Map();
for (let i = 0; i < nodes.length; i += nl) {
  const start = firstEdge[i / nl], end = start + nodes[i + E] * el;
  for (let e = start; e < end; e += el) {
    const to = edges[e + EO];
    if (!retainers.has(to)) retainers.set(to, []);
    const list = retainers.get(to);
    if (list.length < 3) list.push(`${types[nodes[i + T]]}:${strings[nodes[i + N]].slice(0, 40)}.${etypes[edges[e + ET]] === "element" || etypes[edges[e + ET]] === "hidden" ? edges[e + EN] : strings[edges[e + EN]]}`);
  }
}
const big = [];
for (let i = 0; i < nodes.length; i += nl) big.push([nodes[i + S], i]);
big.sort((a, b) => b[0] - a[0]);
for (const [size, i] of big.slice(0, 10)) console.log(`${(size / 1e6).toFixed(2)} MB ${types[nodes[i + T]]}:${strings[nodes[i + N]].slice(0, 50).replace(/\n/g, "⏎")} <- ${(retainers.get(i) ?? []).join(" | ")}`);
// Strings summed by their first retainer, two levels up.
const parentOf = new Map();
for (const [to, list] of retainers) parentOf.set(to, list);
const firstRetainerIndex = new Map();
for (let i = 0; i < nodes.length; i += nl) {
  const start = firstEdge[i / nl], end = start + nodes[i + E] * el;
  for (let e = start; e < end; e += el) { const to = edges[e + EO]; if (!firstRetainerIndex.has(to)) firstRetainerIndex.set(to, i); }
}
const byRetainer = new Map();
for (let i = 0; i < nodes.length; i += nl) {
  if (types[nodes[i + T]] !== "string") continue;
  const p = firstRetainerIndex.get(i);
  const pp = p === undefined ? undefined : firstRetainerIndex.get(p);
  const label = (x) => (x === undefined ? "-" : `${types[nodes[x + T]]}:${strings[nodes[x + N]].slice(0, 30).replace(/\n/g, "⏎")}`);
  const key = `${label(p)} <- ${label(pp)}`;
  byRetainer.set(key, (byRetainer.get(key) ?? 0) + nodes[i + S]);
}
console.log("strings by retainer:");
for (const [k, v] of [...byRetainer].sort((a, b) => b[1] - a[1]).slice(0, 10)) console.log(`${(v / 1e6).toFixed(2).padStart(8)} MB  ${k}`);
// For strings held by plain objects: which property, and the object's other keys.
const byProp = new Map();
for (let i = 0; i < nodes.length; i += nl) {
  if (types[nodes[i + T]] !== "object") continue;
  const start = firstEdge[i / nl], end = start + nodes[i + E] * el;
  const keys = [];
  let held = 0, prop = "";
  for (let e = start; e < end; e += el) {
    const kind = etypes[edges[e + ET]];
    if (kind !== "property") continue;
    const k = strings[edges[e + EN]];
    keys.push(k);
    const to = edges[e + EO];
    const t = types[nodes[to + T]];
    if (t === "string" || t === "concatenated string") { held += nodes[to + S]; prop ||= k; }
  }
  if (!held) continue;
  const key = `{${keys.slice(0, 8).join(",")}}`;
  byProp.set(key, (byProp.get(key) ?? 0) + held);
}
console.log("objects holding strings, by shape:");
for (const [k, v] of [...byProp].sort((a, b) => b[1] - a[1]).slice(0, 8)) console.log(`${(v / 1e6).toFixed(2).padStart(8)} MB  ${k}`);
// The retainer chain of one {html,body,next} object (first retainer each step).
const allRetainers = new Map();
for (let i = 0; i < nodes.length; i += nl) {
  const start = firstEdge[i / nl], end = start + nodes[i + E] * el;
  for (let e = start; e < end; e += el) {
    if (etypes[edges[e + ET]] === "weak") continue;
    const to = edges[e + EO];
    if (!allRetainers.has(to)) allRetainers.set(to, []);
    allRetainers.get(to).push([i, etypes[edges[e + ET]] === "element" || etypes[edges[e + ET]] === "hidden" ? String(edges[e + EN]) : strings[edges[e + EN]]]);
  }
}
const isMemo = (i) => { const start = firstEdge[i / nl], end = start + nodes[i + E] * el; const k = []; for (let e = start; e < end; e += el) if (etypes[edges[e + ET]] === "property") k.push(strings[edges[e + EN]]); return k.filter((x) => x !== "__proto__").join(",") === "html,body,next"; };
let sampleCount = 0;
for (let i = 0; i < nodes.length && sampleCount < 3; i += nl) {
  if (types[nodes[i + T]] !== "object" || !isMemo(i)) continue;
  sampleCount += 1;
  let at = i; const chain = []; const seen = new Set();
  for (let step = 0; step < 14; step += 1) {
    const list = (allRetainers.get(at) ?? []).filter(([p]) => !seen.has(p));
    if (!list.length) break;
    const [p, edge] = list[0]; seen.add(p);
    chain.push(`${types[nodes[p + T]]}:${strings[nodes[p + N]].slice(0, 28).replace(/\n/g, " ")}[${edge}]`);
    at = p;
  }
  console.log("chain:", chain.join(" <- "));
}
