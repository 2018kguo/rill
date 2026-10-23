// DOM differential test page: renders each document with Streamdown 2.7.1 (the
// reference) and with a candidate, waits until both stop changing (async highlighting
// included), and reports both containers' HTML. Driven by dom-diff.mjs.
//
//   dom.html?config=default|code|math|cjk|all&variant=<name>&candidate=self|rill
import { createElement, type ComponentType, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { Streamdown } from "streamdown";
import { cjk } from "@streamdown/cjk";
import { code } from "@streamdown/code";
import { math } from "@streamdown/math";

import { blockRenders, Streamdown as RillStreamdown } from "../../js/src/streamdown.ts";
import { initRill } from "../../js/src/index.ts";

const params = new URLSearchParams(location.search);
const config = params.get("config") ?? "default";
const variant = params.get("variant") ?? "default";
const candidateName = params.get("candidate") ?? "rill";

const plugins =
  config === "code"
    ? { code }
    : config === "math"
      ? { math }
      : config === "cjk"
        ? { cjk }
        : config === "all"
          ? { code, math, cjk }
          : undefined;

function CustomLink({ node: _node, children, ...props }: Record<string, unknown>) {
  return createElement("a", { "data-custom": "link", ...props }, children as ReactNode);
}

// Streamdown props per variant (beyond `plugins` and the document).
const variants: Record<string, Record<string, unknown>> = {
  default: {},
  prefix: { prefix: "tw" },
  controls: { controls: false },
  nosafety: { linkSafety: { enabled: false } },
  nolines: { lineNumbers: false },
  components: { components: { a: CustomLink } },
  rtl: { dir: "rtl" },
  static: { mode: "static" },
  caret: { caret: "block" },
};
const variantProps = variants[variant];
if (!variantProps) throw new Error(`unknown variant ${variant}`);

await initRill(await (await fetch("rill_bg.wasm")).arrayBuffer());

type Component = ComponentType<Record<string, unknown>>;
const candidates: Record<string, Component> = {
  self: Streamdown as never,
  rill: RillStreamdown as never,
};

function settle(element: HTMLElement, quietMs: number, timeoutMs = 15000): Promise<void> {
  return new Promise((resolve) => {
    let timer = setTimeout(done, quietMs);
    const observer = new MutationObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(done, quietMs);
    });
    const deadline = setTimeout(done, timeoutMs);
    observer.observe(element, {
      subtree: true,
      childList: true,
      attributes: true,
      characterData: true,
    });
    function done() {
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

async function renderOne(component: Component, markdown: string, props: Record<string, unknown>) {
  const { host, root } = mount();
  root.render(createElement(component, { plugins, ...variantProps, ...props }, markdown));
  await settle(host, 250);
  const html = host.innerHTML;
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
      createElement(component, { plugins, ...variantProps, isAnimating }, markdown)
    );
    await settle(host, 120);
    out.push(host.innerHTML);
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
page.blockRenders = () => ({ ...blockRenders });
page.probe = async (markdown: string) => ({
  reference: await renderOne(Streamdown as never, markdown, {}),
  actual: await renderOne(candidates[candidateName] as Component, markdown, {}),
});
document.body.dataset.ready = "true";
import { styleAttribute } from "../../js/src/html.ts";
page.styleAttribute = styleAttribute;
