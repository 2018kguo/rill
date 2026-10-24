// DOM differential test page: renders each document with Streamdown 2.7.1 (the
// reference) and with a candidate, waits until both stop changing (async highlighting
// included), and reports both containers' HTML. Driven by dom-diff.mjs.
//
//   dom.html?config=default|code|math|cjk|all&variant=<name>&candidate=self|rill
import "./no-time-limit.ts";
import { createContext, createElement, memo, type ComponentType, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { Block as StreamdownBlock, Streamdown } from "streamdown";
import { cjk } from "@streamdown/cjk";
import { code } from "@streamdown/code";
import { math } from "@streamdown/math";

import {
  Block as RillBlock,
  blockRenders,
  highlightCounts,
  pendingHighlights,
  setHighlightWasm,
  setRillHighlight,
  Streamdown as RillStreamdown,
} from "../../js/src/streamdown.ts";
import { initRill } from "../../js/src/index.ts";

const params = new URLSearchParams(location.search);
const config = params.get("config") ?? "default";
const variant = params.get("variant") ?? "default";
const candidateName = params.get("candidate") ?? "rill";

// The stock code plugin, counting the requests it has not answered: settle() waits for
// them as for rill's worker, since Shiki on a cold grammar can outlast the quiet period
// (the snapshot would show the unhighlighted block). rill still sees the stock plugin
// (it goes by name and type) and swaps in rill-highlight. A request that fails is
// logged and never answered, so one older than 10 s stops counting.
const codeRequests = new Set<{ at: number }>();
const codePending = () => {
  for (const request of codeRequests) if (performance.now() - request.at > 10000) codeRequests.delete(request);
  return codeRequests.size;
};
const countedCode: typeof code = {
  ...code,
  highlight(options, callback) {
    const request = { at: performance.now() };
    codeRequests.add(request);
    const result = code.highlight(options, (highlighted) => {
      codeRequests.delete(request);
      callback?.(highlighted);
    });
    if (result) codeRequests.delete(request);
    return result;
  },
};

const plugins =
  config === "code"
    ? { code: countedCode }
    : config === "math"
      ? { math }
      : config === "cjk"
        ? { cjk }
        : config === "all"
          ? { code: countedCode, math, cjk }
          : undefined;

function OwnCopyIcon(props: Record<string, unknown>) {
  return createElement("svg", { ...props, "data-own": "copy" });
}

function CustomLink({ node: _node, children, ...props }: Record<string, unknown>) {
  return createElement("a", { "data-custom": "link", ...props }, children as ReactNode);
}

// An app's own BlockComponent wrapping the Block it imports (Arca's shape): each side
// wraps its own module's Block.
const IndexContext = createContext(-1);
const wrapBlock = (Block: ComponentType<Record<string, unknown>>) =>
  memo((props: Record<string, unknown>) =>
    createElement(IndexContext.Provider, { value: props.index as number }, createElement(Block, props))
  );
const ReferenceBlock = wrapBlock(StreamdownBlock as never);
const CandidateBlock = wrapBlock(RillBlock as never);

// Streamdown props per variant (beyond `plugins` and the document). `block` is set per
// side below.
const variants: Record<string, Record<string, unknown>> = {
  block: {},
  default: {},
  prefix: { prefix: "tw" },
  controls: { controls: false },
  nosafety: { linkSafety: { enabled: false } },
  nolines: { lineNumbers: false },
  components: { components: { a: CustomLink } },
  rtl: { dir: "rtl" },
  static: { mode: "static" },
  caret: { caret: "block" },
  // The code controls' labels (escaped in their markup), and a copy icon of the page's
  // own (the controls render through React from the start).
  labels: { translations: { copyCode: 'Copier "le" <code>', downloadFile: "Télécharger & garder" } },
  icons: { icons: { CopyIcon: OwnCopyIcon } },
};
const variantProps = variants[variant];
if (!variantProps) throw new Error(`unknown variant ${variant}`);
const sideProps = (component: unknown): Record<string, unknown> =>
  variant === "block"
    ? { BlockComponent: component === Streamdown ? ReferenceBlock : CandidateBlock }
    : {};

await initRill(await (await fetch("rill_bg.wasm")).arrayBuffer());
setHighlightWasm(new URL("rill_highlight.wasm", location.href));
// `?shiki`: the stock plugin highlights (Shiki on the main thread), for comparisons.
if (params.has("shiki")) setRillHighlight(false);

type Component = ComponentType<Record<string, unknown>>;
const candidates: Record<string, Component> = {
  self: Streamdown as never,
  rill: RillStreamdown as never,
};

// The src each image has fired load or error for. An image is `complete` as soon as its
// (aborted) request fails, but the error event, in which Streamdown's image hides itself,
// runs as a later task: a snapshot between the two would miss the `hidden` class.
const firedImages = new WeakMap<HTMLImageElement, string>();
for (const type of ["load", "error"]) {
  document.addEventListener(
    type,
    (event) => {
      if (event.target instanceof HTMLImageElement) firedImages.set(event.target, event.target.src);
    },
    true
  );
}
const imageDone = (image: HTMLImageElement) =>
  image.complete && (!image.getAttribute("src") || firedImages.get(image) === image.src);

// A code block still showing its raw text (Streamdown's Suspense fallback, or its body
// before the highlight lands): every token "inherit". Highlighted tokens always carry
// the theme's colors, so with a code plugin no settled block looks like this.
const UNHIGHLIGHTED = '[data-streamdown="code-block-body"] span[style*="--sdm-c: inherit"]';

/** Resolves once `element` has not changed for `quietMs`, its images are done, and no
 * highlighting is in flight in rill's worker (whose answers can outlast the quiet period). */
function settle(element: HTMLElement, quietMs: number, timeoutMs = 15000): Promise<void> {
  return new Promise((resolve) => {
    let timer = setTimeout(done, quietMs);
    const observer = new MutationObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(done, quietMs);
    });
    const started = performance.now();
    const deadline = setTimeout(done, timeoutMs);
    observer.observe(element, {
      subtree: true,
      childList: true,
      attributes: true,
      characterData: true,
    });
    function done() {
      // An image whose (aborted) request has not failed yet still flips to `hidden`.
      const loading = [...element.querySelectorAll("img")].some((image) => !imageDone(image));
      const unhighlighted = plugins?.code !== undefined && element.querySelector(UNHIGHLIGHTED) !== null;
      if ((loading || unhighlighted || pendingHighlights() > 0 || codePending() > 0) && performance.now() < started + timeoutMs) {
        timer = setTimeout(done, quietMs);
        return;
      }
      observer.disconnect();
      clearTimeout(timer);
      clearTimeout(deadline);
      resolve();
    }
  });
}

function mount(): { host: HTMLElement; root: Root } {
  const host = document.createElement("div");
  document.body.append(host);
  return { host, root: createRoot(host) };
}

/** The host's HTML, and its text nodes' lengths: HTML parsing joins adjacent text that
 * React keeps as separate nodes, which innerHTML does not show. */
function snapshot(host: HTMLElement): string {
  const walker = document.createTreeWalker(host, NodeFilter.SHOW_TEXT);
  const lengths: number[] = [];
  while (walker.nextNode()) lengths.push((walker.currentNode as Text).length);
  return `${host.innerHTML}\n<!-- text nodes: ${lengths.join(" ")} -->`;
}

async function renderOne(component: Component, markdown: string, props: Record<string, unknown>) {
  const { host, root } = mount();
  root.render(
    createElement(
      component,
      { plugins, ...variantProps, ...sideProps(component), ...props },
      markdown
    )
  );
  await settle(host, 250);
  const html = snapshot(host);
  root.unmount();
  host.remove();
  return html;
}

/** Renders `steps` (markdown, isAnimating) one after another into one root, like a
 * streamed answer, recording the settled HTML after each. */
async function renderSteps(component: Component, steps: [string, boolean][]) {
  const { host, root } = mount();
  const out: string[] = [];
  for (const [markdown, isAnimating] of steps) {
    root.render(
      createElement(
        component,
        { plugins, ...variantProps, ...sideProps(component), isAnimating },
        markdown
      )
    );
    await settle(host, 120);
    out.push(snapshot(host));
  }
  root.unmount();
  host.remove();
  return out;
}

const page = window as unknown as Record<string, unknown>;
page.run = async (documents: string[]) => {
  const candidate = candidates[candidateName];
  if (!candidate) throw new Error(`unknown candidate ${candidateName}`);
  const results = [];
  for (const markdown of documents) {
    // Streamdown highlights a language differently on its first, cold render; warm
    // every cache first so both sides see the same state.
    await renderOne(Streamdown as never, markdown, {});
    const reference = await renderOne(Streamdown as never, markdown, {});
    const actual = await renderOne(candidate, markdown, {});
    results.push({ reference, actual });
  }
  return results;
};
page.stream = async (steps: [string, boolean][]) => {
  const candidate = candidates[candidateName];
  if (!candidate) throw new Error(`unknown candidate ${candidateName}`);
  const last = steps.at(-1);
  if (last) await renderOne(Streamdown as never, last[0], {});
  const reference = await renderSteps(Streamdown as never, steps);
  const actual = await renderSteps(candidate, steps);
  return reference.map((html, index) => ({ reference: html, actual: actual[index] }));
};
// Interaction tests (interactions.mjs): mounts a candidate that stays on the page.
page.mount = (markdown: string, props: Record<string, unknown> = {}) => {
  const { host, root } = mount();
  host.id = "mounted";
  root.render(createElement(candidates[candidateName] as Component, { plugins, ...props }, markdown));
};
page.blockRenders = () => ({ ...blockRenders, highlights: highlightCounts() });
page.probe = async (markdown: string) => ({
  reference: await renderOne(Streamdown as never, markdown, {}),
  actual: await renderOne(candidates[candidateName] as Component, markdown, {}),
});
document.body.dataset.ready = "true";
