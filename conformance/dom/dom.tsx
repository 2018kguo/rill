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
import { createMathPlugin, math } from "@streamdown/math";
import rehypeExternalLinks from "rehype-external-links";
import rehypeKatex from "rehype-katex";
import rehypeSlug from "rehype-slug";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import { defaultRehypePlugins, defaultRemarkPlugins } from "streamdown";

import {
  Block as RillBlock,
  blockRenders,
  highlightCounts,
  pendingHighlights,
  setFallbackListener,
  setHighlightWasm,
  setRillHighlight,
  Streamdown as RillStreamdown,
} from "../../js/src/streamdown.ts";
import { initRill } from "../../js/src/index.ts";
import { customEngine } from "../../examples/custom-build/engine.ts";

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

// An app's own BlockComponent wrapping the Block it imports: each side
// wraps its own module's Block.
const IndexContext = createContext(-1);
const wrapBlock = (Block: ComponentType<Record<string, unknown>>) =>
  memo((props: Record<string, unknown>) =>
    createElement(IndexContext.Provider, { value: props.index as number }, createElement(Block, props))
  );
const ReferenceBlock = wrapBlock(StreamdownBlock as never);
const CandidateBlock = wrapBlock(RillBlock as never);

const REMARK = Object.values(defaultRemarkPlugins);
const REHYPE = Object.values(defaultRehypePlugins);

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
  // A custom build (examples/custom-build) against Streamdown with the JavaScript plugins
  // its engine carries; set per side below.
  engine: {},
  "engine-static": { mode: "static" },
  caret: { caret: "block" },
  // The code controls' labels (escaped in their markup), and a copy icon of the page's
  // own (the controls render through React from the start).
  labels: { translations: { copyCode: 'Copier "le" <code>', downloadFile: "Télécharger & garder" } },
  icons: { icons: { CopyIcon: OwnCopyIcon } },
  // Streamdown 2's word animation while streaming (compare with --stream), with its
  // defaults and with every option.
  animated: { animated: true },
  // A page that never passes isAnimating while it streams (compare with --stream): the
  // drop-in cannot tell, and must stay correct and keep memory linear.
  noanim: {},
  // The plugins pages add most (rill builds them in), as pages pass them.
  breaks: { remarkPlugins: [remarkGfm, remarkBreaks] },
  slug: { rehypePlugins: [...REHYPE, rehypeSlug] },
  external: { rehypePlugins: [...REHYPE, [rehypeExternalLinks, { target: "_blank" }]], linkSafety: { enabled: false } },
  "remark-math": { remarkPlugins: [...REMARK, remarkMath], rehypePlugins: [...REHYPE, rehypeKatex] },
  "math-single": { plugins: { ...(config === "code" || config === "all" ? { code: countedCode } : {}), math: createMathPlugin({ singleDollarTextMath: true }) } },
  extras: {
    remarkPlugins: [...REMARK, remarkBreaks, remarkMath],
    rehypePlugins: [...REHYPE, rehypeSlug, rehypeKatex, [rehypeExternalLinks, { target: "_blank", rel: ["noopener"] }]],
    linkSafety: { enabled: false },
  },
  "animated-options": {
    animated: { animation: "blurIn", duration: 320, easing: "cubic-bezier(0.4, 0, 0.2, 1)", sep: "char", stagger: 12, maxBacklogMs: 200 },
  },
};
const variantProps = variants[variant];
if (!variantProps) throw new Error(`unknown variant ${variant}`);
const ENGINE_ARGS: [string, boolean] = ["/users", true];
const sideProps = (component: unknown): Record<string, unknown> =>
  variant === "block"
    ? { BlockComponent: component === Streamdown ? ReferenceBlock : CandidateBlock }
    : variant.startsWith("engine")
      ? component === Streamdown
        ? customEngine.plugins(...ENGINE_ARGS)
        : { engine: customEngine, engineArgs: ENGINE_ARGS }
      : {};

await initRill(await (await fetch("rill_bg.wasm")).arrayBuffer());
if (variant.startsWith("engine")) await customEngine.init(await (await fetch("custom_bg.wasm")).arrayBuffer());
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
/** When the last image event fired: the component hides an image in a React update that
 * commits in a later task, so a settled DOM is also quiet since then. */
let lastImageEvent = 0;
for (const type of ["load", "error"]) {
  document.addEventListener(
    type,
    (event) => {
      if (!(event.target instanceof HTMLImageElement)) return;
      firedImages.set(event.target, event.target.src);
      lastImageEvent = performance.now();
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
      const loading =
        [...element.querySelectorAll("img")].some((image) => !imageDone(image)) ||
        performance.now() - lastImageEvent < quietMs;
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

// With `animated`, delays come from the clock: both sides see the same one, advanced a
// fixed step per update.
let clock = 0;
if (variant.startsWith("animated")) performance.now = () => clock;

/** Renders `steps` (markdown, isAnimating) one after another into one root, like a
 * streamed answer, recording the settled HTML after each. */
async function renderSteps(component: Component, steps: [string, boolean][]) {
  const { host, root } = mount();
  const out: string[] = [];
  clock = 1000;
  for (const [markdown, isAnimating] of steps) {
    clock += 23;
    root.render(
      createElement(
        component,
        { plugins, ...variantProps, ...sideProps(component), ...(variant === "noanim" ? {} : { isAnimating }) },
        markdown
      )
    );
    await settle(host, 120);
    out.push(snapshot(host));
  }
  // The heap the streamed answer holds while still mounted (dom-diff's --max-heap).
  await (window as unknown as { heapProbe?: () => Promise<void> }).heapProbe?.();
  root.unmount();
  host.remove();
  return out;
}

// Why blocks went to Streamdown's own Block, by reason.
const fallbacks: Record<string, number> = {};
setFallbackListener((reason) => {
  fallbacks[reason] = (fallbacks[reason] ?? 0) + 1;
});

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
page.blockRenders = () => ({ ...blockRenders, highlights: highlightCounts(), fallbacks });
page.probe = async (markdown: string) => ({
  reference: await renderOne(Streamdown as never, markdown, {}),
  actual: await renderOne(candidates[candidateName] as Component, markdown, {}),
});
document.body.dataset.ready = "true";
