import { toHtml } from "hast-util-to-html";

import { pipeline } from "../conformance/pipeline.mjs";
import { createRill } from "../js/src/core.ts";
import init, { renderHast } from "../js/wasm/rill.js";
import emphasis from "../examples/01-emphasis.md" with { type: "text" };
import containers from "../examples/02-containers.md" with { type: "text" };
import tables from "../examples/03-tables.md" with { type: "text" };
import links from "../examples/04-links.md" with { type: "text" };
import codeMath from "../examples/05-code-math.md" with { type: "text" };
import html from "../examples/06-html.md" with { type: "text" };
import streaming from "../examples/07-streaming.md" with { type: "text" };

const examples = {
  "Long answer (pure Markdown)": [emphasis, containers, tables, links, streaming].join("\n\n"),
  "Everything (with math and HTML)": [emphasis, containers, tables, links, codeMath, html, streaming].join("\n\n"),
  "Emphasis, CJK, strikethrough": emphasis,
  "Nested containers, tasks": containers,
  "Tables": tables,
  "Links, autolinks, harden": links,
  "Code and math": codeMath,
  "Raw HTML, sanitize": html,
  "Half-streamed answer": streaming,
};

const $ = (id) => document.getElementById(id);

/** Runs `render` repeatedly for ~60 ms and returns the mean per-call time (browsers
 * coarsen `performance.now()`, so single calls can't be timed). */
function time(render) {
  let result = render();
  let runs = 0;
  const start = performance.now();
  let now = start;
  while (now - start < 60 && runs < 500) {
    result = render();
    runs += 1;
    now = performance.now();
  }
  return { result, ms: (now - start) / runs };
}

// Streamdown with `plugins={{ math, cjk }}`: the examples have both.
const { processor, tails } = pipeline("math+cjk");
const javascript = (source) => processor.runSync(processor.parse(source), source);
const PATHS = {
  complete: "all in Rust",
  raw: "+ JS tail for raw HTML",
  math: "+ JS tail for KaTeX",
};
let lastKind = "complete";
// The npm package's own path: opcodes from WebAssembly, decoded into HAST.
const engine = createRill(renderHast);
function rill(source) {
  const rendered = engine.render(source, { math: true, cjk: true });
  if (!rendered) return javascript(source);
  const { kind, tree } = rendered;
  lastKind = kind;
  const tail = tails[kind];
  return tail ? tail.runSync(tree, source) : tree;
}

const plain = (tree) =>
  JSON.stringify(tree, (key, value) => (key === "data" ? undefined : value));

function render(source, measure = true) {
  const js = measure ? time(() => javascript(source)) : { result: javascript(source) };
  const rs = measure ? time(() => rill(source)) : { result: rill(source) };
  $("js-out").innerHTML = toHtml(js.result);
  $("rs-out").innerHTML = toHtml(rs.result);
  const same = plain(js.result) === plain(rs.result);
  $("badge").textContent = same ? "identical HAST" : "HAST differs";
  $("badge").className = `badge ${same ? "ok" : "bad"}`;
  $("path").textContent = PATHS[lastKind];
  $("diff").hidden = same;
  if (!same) $("diff").textContent = "Trees differ: see the console for both.";
  if (!same) console.log({ javascript: js.result, rill: rs.result });
  if (measure) {
    $("js").textContent = `${js.ms.toFixed(3)} ms`;
    $("rs").textContent = `${rs.ms.toFixed(3)} ms`;
    $("x").textContent = `${(js.ms / rs.ms).toFixed(1)}×`;
  }
}

/** Replays the source a few characters per frame, like a model streaming its answer. */
async function stream() {
  const source = $("source").value;
  let js = 0;
  let rs = 0;
  for (let end = 0; end <= source.length + 11; end += 12) {
    const prefix = source.slice(0, end);
    let start = performance.now();
    const jsTree = javascript(prefix);
    js += performance.now() - start;
    start = performance.now();
    const rsTree = rill(prefix);
    rs += performance.now() - start;
    $("js-out").innerHTML = toHtml(jsTree);
    $("rs-out").innerHTML = toHtml(rsTree);
    $("js").textContent = `${js.toFixed(1)} ms total`;
    $("rs").textContent = `${rs.toFixed(1)} ms total`;
    $("x").textContent = `${(js / rs).toFixed(1)}×`;
    await new Promise((resolve) => setTimeout(resolve, 16));
  }
  render(source);
}

await init();
for (const name of Object.keys(examples)) $("example").add(new Option(name, name));
const show = () => {
  $("source").value = examples[$("example").value];
  render($("source").value);
};
$("example").addEventListener("change", show);
$("source").addEventListener("input", () => render($("source").value));
$("stream").addEventListener("click", stream);
show();
