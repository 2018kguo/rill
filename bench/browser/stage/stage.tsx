// The recording stage: one engine (`./engine.ts`, which build.ts points at
// engine.streamdown.ts, engine.rill.ts or a file of your own, exports a `Streamdown`
// component and `answerProps(settled, text)`, the props each answer adds), production
// React, real content, a chat-style page that follows the newest text.
//   stage.html?scene=stream&doc=lru-cache&rate=1200  |  stage.html?scene=history
// The overlay is written outside React: frames painted in the last second, the worst
// frame gap in the last second, elapsed time, and when everything was on screen.
// `window.go()` starts the scene (both windows of a side-by-side recording at once).
import { createElement, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { answerProps, code, engineName, Streamdown } from "./engine.ts";
import elements from "./elements.md" with { type: "text" };
import japan from "./japan.md" with { type: "text" };
import lruCache from "./lru-cache.md" with { type: "text" };
// The Rust Book's "What Is Ownership?" (rust-lang/book, MIT/Apache-2.0), listings inlined.
import ownership from "./ownership.md" with { type: "text" };
// The whole chapter 4, "Understanding Ownership" (4.0–4.3).
import ownershipChapter from "./ownership-chapter.md" with { type: "text" };
// All 118 elements, derived from the Periodic-Table-JSON dataset by Bowserinator (CC BY-SA 3.0).
import elements118 from "./elements-118.md" with { type: "text" };
import ownershipNodef from "./ownership-nodef.md" with { type: "text" };
import ownershipFn from "./ownership-fn.md" with { type: "text" };
// chat=distinct: an 18-turn conversation where no two answers are the same (15 Rust
// Book sections, listings inlined, plus the three general answers).
import chat from "./chat.json";

const documents: Record<string, string> = { elements, japan, "lru-cache": lruCache, ownership, "ownership-chapter": ownershipChapter, "elements-118": elements118, "ownership-nodef": ownershipNodef, "ownership-fn": ownershipFn };
const params = new URLSearchParams(location.search);
const scene = params.get("scene") ?? "stream";
const rate = Number(params.get("rate") ?? 1200);
// chunk > 0: reveal that many characters per frame regardless of time, so the engine
// is the bottleneck and the run's length measures its throughput.
const chunk = Number(params.get("chunk") ?? 0);
const turns = Number(params.get("turns") ?? 18);
const source = documents[params.get("doc") ?? "lru-cache"] ?? lruCache;
// nocode=1: no highlighting (prices what highlighting adds to a run).
const plugins = params.get("nocode") === "1" ? {} : { code };

const $ = (id: string) => document.getElementById(id)!;
const frames: number[] = [];
const stats = { started: 0, onScreen: 0, revealed: 0, finished: 0, visible: 0 };
// Long Animation Frames: the browser's own per-frame breakdown of slow frames.
const loaf: Record<string, number>[] = [];
try {
  new PerformanceObserver((list) => {
    for (const entry of list.getEntries() as unknown as Record<string, number>[]) {
      loaf.push({ startTime: entry.startTime, duration: entry.duration, blockingDuration: entry.blockingDuration, renderStart: entry.renderStart, styleAndLayoutStart: entry.styleAndLayoutStart });
    }
  }).observe({ type: "long-animation-frame", buffered: true });
} catch {}
(window as unknown as { stage: unknown }).stage = { stats, frames, engine: engineName, loaf };
// Trace markers for the benchmark driver (blink.user_timing).
const begin = () => performance.mark("stage:start");
const end = () => performance.mark("stage:end");

// Done once the DOM has been still for 600 ms after `ready()`, every code block is
// highlighted, and the main thread was free for those 600 ms (a frame gap over 100 ms
// restarts the wait: a long task can be highlighting that has not reached the DOM yet);
// `finished` is the last mutation.
// Raw (not yet highlighted) lines are one token colored `inherit`.
// lazy=1: code off screen may stay raw (an engine with lazyHighlight); only bodies in the
// viewport must be highlighted. The quiet rule still waits out any engine's other work.
const rawSelector = '#answer [data-streamdown="code-block-body"] span[style*="--sdm-c: inherit"]';
const inView = (element: Element) => {
  const view = $("scroll").getBoundingClientRect();
  const box = element.getBoundingClientRect();
  return box.bottom > view.top && box.top < view.bottom;
};
const highlighted = () =>
  params.get("nocode") === "1" ||
  (params.get("lazy") === "1"
    ? ![...document.querySelectorAll(rawSelector)].some(inView)
    : !document.querySelector(rawSelector));
function settle(ready: () => boolean) {
  let lastMutation = performance.now();
  let quietSince = lastMutation;
  let lastTick = lastMutation;
  // Every mutation's time and target, to find the last one a reader could see (`visible`:
  // the last change inside the viewport once the run ends, as "visually complete").
  const changes: [number, Node][] = [];
  new MutationObserver((records) => {
    lastMutation = performance.now();
    for (const record of records) changes.push([lastMutation, record.target]);
  }).observe($("answer"), { subtree: true, childList: true, attributes: true, characterData: true });
  const lastVisible = () => {
    const view = $("scroll").getBoundingClientRect();
    let last = 0;
    for (const [time, target] of changes) {
      if (time <= last) continue;
      const element = target instanceof Element ? target : target.parentElement;
      if (!element?.isConnected) continue;
      const box = element.getBoundingClientRect();
      if (box.bottom > view.top && box.top < view.bottom && box.right > view.left && box.left < view.right) last = time;
    }
    return last;
  };
  const tick = () => {
    const now = performance.now();
    if (now - lastTick > 100) quietSince = now;
    lastTick = now;
    if (ready() && now - Math.max(lastMutation, quietSince) > 600 && highlighted()) {
      stats.finished = lastMutation;
      overlay(now);
      end();
      stats.visible = lastVisible();
      document.body.dataset.done = "true";
      dispatchEvent(new Event("stage-done"));
      return;
    }
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

// overlay=0 for benchmarks: the counter's own repaint is harness cost, not the engine's.
const showOverlay = params.get("overlay") !== "0";
function overlay(now: number) {
  if (!showOverlay) return;
  const recent = frames.filter((time) => now - time < 1000);
  let worst = 0;
  for (let index = 1; index < recent.length; index += 1) worst = Math.max(worst, recent[index]! - recent[index - 1]!);
  $("fps").textContent = String(recent.length);
  $("worst").textContent = worst.toFixed(0);
  const end = stats.finished || now;
  $("elapsed").textContent = stats.started ? ((end - stats.started) / 1000).toFixed(1) : "0.0";
}

function follow() {
  const scroller = $("scroll");
  scroller.scrollTop = scroller.scrollHeight;
}

function Stream() {
  const [shown, setShown] = useState("");
  const [done, setDone] = useState(false);
  useEffect(() => {
    let frame = 0;
    const tick = () => {
      const now = performance.now();
      frames.push(now);
      revealed = Math.min(source.length, chunk ? revealed + chunk : Math.floor(((now - stats.started) * rate) / 1000));
      setShown(source.slice(0, revealed));
      follow();
      overlay(now);
      if (revealed < source.length) frame = requestAnimationFrame(tick);
      else {
        stats.revealed = now;
        setDone(true);
      }
    };
    let revealed = 0;
    (window as unknown as { go: () => void }).go = () => {
      begin();
      stats.started = performance.now();
      frame = requestAnimationFrame(tick);
      settle(() => stats.revealed > 0);
    };
    return () => cancelAnimationFrame(frame);
  }, []);
  // noanim=1: a page that never passes isAnimating (the drop-in cannot tell it streams).
  const animating = params.get("noanim") === "1" ? {} : { isAnimating: !done };
  return createElement(Streamdown, { plugins, ...answerProps(done, shown), ...animating }, shown);
}

const asked: Record<string, string> = {
  "lru-cache": "How do I implement an LRU cache?",
  elements: "List the first 50 elements",
  japan: "Plan 10 days in Japan",
  ownership: "Explain Rust ownership",
};
// answers=a,b,c picks the conversation's answers in turn (default: a mixed chat).
const picked = (params.get("answers") ?? "lru-cache,elements,japan").split(",");
const distinct = params.get("chat") === "distinct";
const questions = distinct ? chat.map((turn) => turn.question) : picked.map((name) => asked[name] ?? name);
const answers = distinct ? chat.map((turn) => turn.answer) : picked.map((name) => documents[name]!);
function History() {
  return createElement(
    "div",
    null,
    Array.from({ length: turns }, (_, index) =>
      createElement(
        "div",
        { key: index, className: "turn" },
        createElement("p", { className: "question" }, questions[index % questions.length]),
        createElement(Streamdown, { plugins, ...answerProps(true, answers[index % answers.length]!) }, answers[index % answers.length]),
      ),
    ),
  );
}

function startHistory() {
  begin();
  stats.started = performance.now();
  createRoot($("answer")).render(createElement(History));
  // Content is on screen once every answer's tables exist.
  settle(() => {
    if (!stats.onScreen && document.querySelectorAll("#answer .turn").length === turns && document.querySelectorAll("#answer [data-streamdown]").length > 0) stats.onScreen = performance.now();
    return stats.onScreen > 0;
  });
  const tick = () => {
    const now = performance.now();
    frames.push(now);
    // bottom=1: the chat opens at its last answer, as chat apps do.
    if (params.get("bottom") === "1") follow();
    overlay(now);
    if (!stats.finished) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

// cv=1: settled turns skip layout and paint while off screen (a page's own CSS; the DOM
// stays the same).
if (params.get("cv") === "1") document.head.append(Object.assign(document.createElement("style"), { textContent: ".turn{content-visibility:auto;contain-intrinsic-size:auto 1200px}" }));
$("engine").textContent = engineName;
document.body.dataset.engine = engineName.includes("rill") ? "rill" : "streamdown";
if (scene === "history") {
  (window as unknown as { go: () => void }).go = startHistory;
  document.body.dataset.ready = "true";
} else if (scene === "after") {
  // A chat that already holds `turns` settled answers (mounted before the run, untimed),
  // then one more answer streams below them: what a chat page does all day.
  createRoot($("answer")).render(createElement("div", null, createElement(History), createElement("div", { className: "turn" }, createElement(Stream))));
  // Ready once the history stopped changing (highlighting included) for 500 ms.
  let quiet = performance.now();
  new MutationObserver(() => (quiet = performance.now())).observe($("answer"), { subtree: true, childList: true, characterData: true, attributes: true });
  const wait = () => (performance.now() - quiet > 500 ? (document.body.dataset.ready = "true") : setTimeout(wait, 100));
  setTimeout(wait, 100);
} else {
  createRoot($("answer")).render(createElement(Stream));
  document.body.dataset.ready = "true";
}
