// Main-thread cost of <Streamdown> (Streamdown 2.7.1) against rill's drop-in, timed
// from outside React (production React has no Profiler callbacks): when the content is
// on screen (the first animation frame with it in the DOM) and when the DOM is quiet
// (highlighting included). Driven by perf.mjs at a CPU throttle.
//
//   perf.html?engine=streamdown|rill|streamdown-block|rill-block&scene=history|table|code
//
// `history` mounts a 20-message conversation at once (opening a chat); `table` and
// `code` stream a long table and a long code answer, one prefix per frame.
import { createContext, createElement, memo, useEffect, useState, type ComponentType } from "react";
import { createRoot } from "react-dom/client";
import { Block as StreamdownBlock, Streamdown } from "streamdown";
import { code } from "@streamdown/code";

import { initRill } from "../../js/src/index.ts";
import { Block as RillBlock, Streamdown as RillStreamdown } from "../../js/src/streamdown.ts";
import answer from "../race/answer.md" with { type: "text" };
import codeAnswer from "./code-answer.md" with { type: "text" };

const params = new URLSearchParams(location.search);
const engine = params.get("engine") ?? "streamdown";
const scene = params.get("scene") ?? "history";
const step = Number(params.get("step") ?? 24);

await initRill(await (await fetch("rill_bg.wasm")).arrayBuffer());

// An app's own BlockComponent around the Block it imports.
const IndexContext = createContext(-1);
const wrap = (Block: ComponentType<Record<string, unknown>>) =>
  memo((props: Record<string, unknown>) =>
    createElement(IndexContext.Provider, { value: props.index as number }, createElement(Block, props))
  );
const engines: Record<string, [ComponentType<Record<string, unknown>>, Record<string, unknown>]> = {
  streamdown: [Streamdown as never, {}],
  rill: [RillStreamdown as never, {}],
  "streamdown-block": [Streamdown as never, { BlockComponent: wrap(StreamdownBlock as never) }],
  "rill-block": [RillStreamdown as never, { BlockComponent: wrap(RillBlock as never) }],
};
const [Component, extra] = engines[engine] ?? engines.streamdown!;
const plugins = { code };

const gpus = ["H100 SXM", "H200", "B200", "A100 80GB", "A100 40GB", "L40S", "L4", "A10G", "RTX 6000 Ada", "MI300X", "MI250X", "Gaudi 3", "T4", "V100", "RTX 4090"];
const clouds = ["AWS", "GCP", "Azure", "Lambda", "CoreWeave", "RunPod"];
const rows = Array.from({ length: 120 }, (_, index) => {
  const gpu = gpus[index % gpus.length];
  const cloud = clouds[index % clouds.length]!;
  const price = (0.6 + ((index * 37) % 97) / 10).toFixed(2);
  return `| ${gpu} | ${cloud} | $${price}/hr | ${16 + ((index * 5) % 176)} GB<br>HBM | **${(index % 4) + 1}×** NVLink | [pricing](https://example.com/${cloud.toLowerCase()}/${index}) |`;
});
const table = `Here are the GPU instances I found, cheapest first within each family:\n\n## GPU instances compared\n\n| GPU | Cloud | On-demand | Memory | Interconnect | Source |\n|---|---|---|---|---|---|\n${rows.join("\n")}\n\nPrices are list prices for on-demand capacity in us-east regions.\n`;

const stats: Record<string, number> = {};
const perf = { stats, start: () => {} };
(window as unknown as { perf: unknown }).perf = perf;
const root = document.getElementById("root") as HTMLElement;
let lastMutation = 0;
new MutationObserver(() => {
  lastMutation = performance.now();
}).observe(root, { subtree: true, childList: true, attributes: true, characterData: true });

/** Spins for 3 ms so a CPU profile can find the moment the content appeared. */
function contentMarker() {
  const end = performance.now() + 3;
  while (performance.now() < end) {
    // Busy on purpose.
  }
}

/** Resolves on the first frame where `ready()` holds, then once the DOM is quiet. */
function watch(ready: () => boolean, done: () => void) {
  const tick = () => {
    const now = performance.now();
    if (!stats.content && ready()) {
      stats.content = now - stats.start!;
      console.timeStamp("perf-content");
      if (params.has("marker")) contentMarker();
    }
    if (stats.content && now - lastMutation > 600) {
      stats.settled = lastMutation - stats.start!;
      console.timeStamp("perf-done");
      done();
      return;
    }
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

function History() {
  return createElement(
    "div",
    null,
    Array.from({ length: 20 }, (_, index) =>
      createElement(
        "div",
        { key: index },
        createElement("p", null, "How should I rate-limit a public API?"),
        createElement(Component, { plugins, ...extra }, answer)
      )
    )
  );
}

function Stream({ source }: { source: string }) {
  const [length, setLength] = useState(0);
  useEffect(() => {
    if (length >= source.length) {
      stats.streamed ??= performance.now() - stats.start!;
      return;
    }
    // One prefix per frame, after the previous one rendered.
    const frame = requestAnimationFrame(() => setLength((value) => Math.min(source.length, value + step)));
    return () => cancelAnimationFrame(frame);
  }, [length, source]);
  const done = length >= source.length;
  return createElement(Component, { plugins, ...extra, isAnimating: !done }, source.slice(0, length));
}

const finish = () => {
  document.body.dataset.done = "true";
};
// The driver starts the scene once it has throttled the CPU.
perf.start = () => {
  stats.start = performance.now();
  console.timeStamp("perf-start");
  if (scene === "history") {
    createRoot(root).render(createElement(History));
    const tables = 20 * 2;
    const codeBlocks = 20 * 3;
    // Live collections: counting them each frame costs next to nothing.
    const tableElements = root.getElementsByTagName("table");
    const codeElements = root.getElementsByTagName("pre");
    watch(() => tableElements.length >= tables && codeElements.length >= codeBlocks, finish);
  } else {
    const source = scene === "table" ? table : codeAnswer;
    stats.updates = Math.ceil(source.length / step);
    createRoot(root).render(createElement(Stream, { source }));
    watch(() => stats.streamed !== undefined, finish);
  }
};
document.body.dataset.ready = "true";
