// Drives dom.html in Chromium: Streamdown 2.7.1 against a candidate, document by
// document, comparing the settled DOM (innerHTML) of each.
//
//   node conformance/dom/dom-diff.mjs [--build] [--config default] [--variant default]
//        [--candidate rill] [--corpus examples,real,chat] [--stream every=40,short=1]
//        [--show 5]
//
// `--build` bundles dom.tsx first. Corpora: `examples` (examples/*.md), `real`
// (conformance/real.jsonl), `chat` (conformance/dom/chat/*.md), or a .jsonl path.
// `--stream` renders prefixes of the chat corpus with isAnimating into one root, as a
// streamed answer arrives: every Nth character of each document (`every`), and every
// prefix of the shortest ones (`short` of them).
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { dirname, extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "../..");
const dist = join(here, "dist");
const args = process.argv.slice(2);
const option = (name, fallback) => {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : fallback;
};
const config = option("--config", "default");
const variant = option("--variant", "default");
const candidate = option("--candidate", "rill");
const show = Number(option("--show", "5"));

if (args.includes("--build") || !existsSync(join(dist, "dom.js"))) buildAll();

function buildAll() {
  // The page, and rill's highlighting worker beside it (js/src/highlight.ts loads it
  // from next to the bundle).
  for (const entry of [join(here, "dom.tsx"), join(here, "../../js/src/highlight-worker.ts")]) build(entry);
  copyFileSync(join(here, "dom.html"), join(dist, "dom.html"));
  copyFileSync(join(repo, "js/wasm/rill_bg.wasm"), join(dist, "rill_bg.wasm"));
  copyFileSync(join(repo, "examples/custom-build/pkg/custom_bg.wasm"), join(dist, "custom_bg.wasm"));
  copyFileSync(join(repo, "node_modules/@2018kguo/rill-highlight/dist/rill_highlight.wasm"), join(dist, "rill_highlight.wasm"));
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
}

function corpus(name) {
  if (name === "examples" || name === "chat" || (existsSync(name) && statSync(name).isDirectory())) {
    const directory = name === "chat" ? join(here, "chat") : name === "examples" ? join(repo, "examples") : name;
    return readdirSync(directory)
      .filter((file) => file.endsWith(".md"))
      .sort()
      .map((file) => readFileSync(join(directory, file), "utf8"));
  }
  const path =
    name === "real" ? join(repo, "conformance/real.jsonl")
    : name;
  if (path.endsWith(".md")) return [readFileSync(path, "utf8")];
  return readFileSync(path, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line));
}

const types = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".wasm": "application/wasm" };
const server = createServer(async (request, response) => {
  const path = join(dist, decodeURIComponent(new URL(request.url, "http://x").pathname));
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

const browser = await chromium.launch();
const jobs = Number(option("--jobs", "6"));
const url = `http://localhost:${server.address().port}/dom.html?config=${config}&variant=${variant}&candidate=${candidate}`;
async function openPage() {
  const page = await browser.newPage();
  // No network beyond the harness: remote images would load at random times.
  await page.route(/^(?!http:\/\/localhost)/, (route) => route.abort());
  page.on("pageerror", (error) => console.error("pageerror:", error.message));
  page.on("console", (message) => {
    const text = message.text();
    if ((message.type() === "error" || message.type() === "warning") && !text.startsWith("Failed to load resource")) {
      console.error("console:", text);
    }
  });
  await page.goto(url);
  await page.waitForSelector("body[data-ready]", { state: "attached" });
  return page;
}
const pages = await Promise.all(Array.from({ length: jobs }, openPage));
const page = pages[0];

/** Runs `window[name](item)` for every item, spread over the pages, in order. */
async function each(name, items) {
  const results = new Array(items.length);
  let next = 0;
  await Promise.all(pages.map(async (worker) => {
    while (next < items.length) {
      const index = next++;
      results[index] = await worker.evaluate(([fn, item]) => window[fn](item), [name, items[index]]);
    }
  }));
  return results;
}

const firstDifference = (a, b) => {
  let index = 0;
  while (index < a.length && a[index] === b[index]) index += 1;
  return index;
};
let failed = 0;
function report(label, results, describe) {
  const failures = [];
  results.forEach(({ reference, actual }, index) => {
    if (reference !== actual) failures.push({ index, reference, actual });
  });
  failed += failures.length;
  console.log(
    `${results.length - failures.length}/${results.length} identical (${label}; config ${config}, variant ${variant}, candidate ${candidate})`
  );
  for (const { index, reference, actual } of failures.slice(0, show)) {
    const at = firstDifference(reference, actual);
    console.log(`  #${index} ${describe(index)}`);
    console.log(`    reference …${reference.slice(Math.max(0, at - 100), at + 100)}`);
    console.log(`    actual    …${actual.slice(Math.max(0, at - 100), at + 100)}`);
  }
}

const stream = option("--stream");
const probe = option("--probe");
if (probe !== undefined) {
  // Prints both sides' HTML for one document (JSON string or plain text).
  const markdown = probe.startsWith('"') ? JSON.parse(probe) : probe;
  const { reference, actual } = await page.evaluate((text) => window.probe(text), markdown);
  console.log(`reference: ${reference}\nactual:    ${actual}\n${reference === actual ? "identical" : "DIFFERENT"}`);
} else if (stream) {
  const settings = Object.fromEntries(stream.split(",").map((pair) => pair.split("=")));
  const every = Number(settings.every ?? 40);
  const documents = corpus(option("--corpus", "chat"));
  const short = [...documents].sort((a, b) => a.length - b.length).slice(0, Number(settings.short ?? 1));
  const runs = [];
  for (const document of documents) {
    const steps = [];
    const all = short.includes(document);
    for (let end = 1; end <= document.length; end += all ? 1 : every) {
      steps.push([document.slice(0, end), true]);
    }
    steps.push([document, true], [document, false]);
    runs.push({ document, steps });
  }
  const outcomes = await each("stream", runs.map((run) => run.steps));
  for (const [index, { document, steps }] of runs.entries()) {
    const results = outcomes[index];
    report(`stream of ${document.length} chars`, results, (index) =>
      `prefix ${steps[index][0].length}${steps[index][1] ? "" : " (done)"}: …${JSON.stringify(steps[index][0].slice(-60))}`
    );
  }
} else {
  for (const name of option("--corpus", "examples,real,chat").split(",")) {
    const documents = corpus(name);
    const results = (await each("run", documents.map((document) => [document]))).flat();
    report(name, results, (index) => JSON.stringify(documents[index]).slice(0, 100));
  }
}
const renders = { rill: 0, streamdown: 0, tableTails: 0, rillHighlight: 0, shiki: 0 };
for (const worker of pages) {
  const counts = await worker.evaluate(() => window.blockRenders());
  renders.rill += counts.rill;
  renders.streamdown += counts.streamdown;
  renders.tableTails += counts.tableTails;
  renders.rillHighlight += counts.highlights.rill;
  renders.shiki += counts.highlights.shiki;
}
console.log(
  `block renders: ${renders.rill} by rill (${renders.tableTails} as a table's new rows), ` +
    `${renders.streamdown} by Streamdown's Block; ` +
    `highlights: ${renders.rillHighlight} by rill-highlight, ${renders.shiki} by Shiki`
);
await browser.close();
server.close();
process.exitCode = failed ? 1 : 0;
