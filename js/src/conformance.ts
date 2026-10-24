// `@2018kguo/rill/conformance`: checks a custom build against the JavaScript plugins its
// engine says it reproduces, the way a page sees them: each document rendered by
// Streamdown 2.7.1 with those plugins and by rill's `<Streamdown engine>`, the settled
// DOMs compared (HTML and where text nodes break). Run it wherever React can render
// into a DOM (happy-dom, jsdom, a browser), with the engine's WebAssembly loaded.
import { createElement, type ComponentType } from "react";
import { flushSync } from "react-dom";
import { createRoot } from "react-dom/client";
import { Streamdown as StreamdownComponent } from "streamdown";

import type { Args } from "./core.ts";
import type { Engine } from "./engine.ts";
import { blockRenders, Streamdown } from "./streamdown.ts";

export type EngineCheckOptions<A extends Args> = {
  engine: Engine<A>;
  /** The build's arguments, as `engineArgs`. */
  args: A;
  documents: readonly string[];
  /** `<Streamdown>` props to render each document with, one render per entry (default:
   * streaming and `mode="static"`). */
  variants?: readonly Record<string, unknown>[];
  /** Also renders every `stream`-th prefix of each document with `isAnimating`, into one
   * root, as an answer streams. */
  stream?: number;
  /** How long the DOM must stay unchanged to count as settled (ms, default 50). */
  quietMs?: number;
};

export type EngineDifference = {
  document: string;
  props: Record<string, unknown>;
  reference: string;
  actual: string;
};

export type EngineReport = {
  /** Renders compared, and how many were identical. */
  renders: number;
  identical: number;
  differences: EngineDifference[];
  /** Blocks rill rendered, and blocks it left to Streamdown's own Block (each one exact,
   * but without rill's speed). */
  rillBlocks: number;
  streamdownBlocks: number;
};

type Component = ComponentType<Record<string, unknown>>;

/** The host's HTML, and its text nodes' lengths: HTML parsing joins adjacent text that
 * React keeps as separate nodes, which innerHTML does not show. */
function snapshot(host: HTMLElement): string {
  const lengths: number[] = [];
  const walk = (node: Node) => {
    for (let child = node.firstChild; child; child = child.nextSibling) {
      if (child.nodeType === 3) lengths.push((child as Text).length);
      else walk(child);
    }
  };
  walk(host);
  return `${host.innerHTML}\n<!-- text nodes: ${lengths.join(" ")} -->`;
}

/** Resolves once `host` has not changed for `quietMs` (at most 10 s). */
function settle(host: HTMLElement, quietMs: number): Promise<void> {
  return new Promise((resolve) => {
    const observer = new MutationObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(done, quietMs);
    });
    let timer = setTimeout(done, quietMs);
    const deadline = setTimeout(done, 10_000);
    observer.observe(host, { subtree: true, childList: true, attributes: true, characterData: true });
    function done() {
      observer.disconnect();
      clearTimeout(timer);
      clearTimeout(deadline);
      resolve();
    }
  });
}

/** Renders `steps` one after another into one root, recording the settled DOM after
 * each. */
async function renderSteps(
  component: Component,
  props: Record<string, unknown>,
  steps: readonly [string, boolean][],
  quietMs: number
): Promise<string[]> {
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  const out: string[] = [];
  for (const [markdown, isAnimating] of steps) {
    flushSync(() => {
      root.render(createElement(component, { ...props, isAnimating }, markdown));
    });
    await settle(host, quietMs);
    out.push(snapshot(host));
  }
  flushSync(() => root.unmount());
  host.remove();
  return out;
}

/** Renders `documents` with Streamdown and the engine's plugins, and with rill's
 * `<Streamdown engine>`, and compares the DOMs. */
export async function checkEngine<A extends Args>(
  options: EngineCheckOptions<A>
): Promise<EngineReport> {
  const { engine, args, documents, stream, quietMs = 50 } = options;
  const variants = options.variants ?? [{}, { mode: "static" }];
  if (!engine.rill(...args)) {
    throw new Error("checkEngine: load the engine first (`await engine.init(bytes)`)");
  }
  const pipeline = engine.plugins(...args);
  const report: EngineReport = {
    renders: 0,
    identical: 0,
    differences: [],
    rillBlocks: 0,
    streamdownBlocks: 0,
  };
  for (const variant of variants) {
    const reference = { ...variant, ...pipeline };
    const candidate = { ...variant, engine, engineArgs: args };
    for (const document of documents) {
      const steps: [string, boolean][] = [];
      if (stream) {
        for (let end = 1; end < document.length; end += stream) {
          steps.push([document.slice(0, end), true]);
        }
        steps.push([document, true]);
      }
      steps.push([document, false]);
      // Streamdown's lazy parts (its code block's body) load on a first render.
      await renderSteps(StreamdownComponent as Component, reference, steps.slice(-1), quietMs);
      const expected = await renderSteps(StreamdownComponent as Component, reference, steps, quietMs);
      const before = { ...blockRenders };
      const actual = await renderSteps(Streamdown as Component, candidate, steps, quietMs);
      report.rillBlocks += blockRenders.rill - before.rill;
      report.streamdownBlocks += blockRenders.streamdown - before.streamdown;
      for (const [index, html] of actual.entries()) {
        report.renders += 1;
        if (html === expected[index]) {
          report.identical += 1;
        } else {
          report.differences.push({
            document: steps[index]![0],
            props: { ...variant, isAnimating: steps[index]![1] },
            reference: expected[index]!,
            actual: html,
          });
        }
      }
    }
  }
  return report;
}
