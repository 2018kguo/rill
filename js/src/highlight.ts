// Highlighting for the drop-in's code blocks. With Streamdown's stock code plugin
// (`@streamdown/code`), rill-highlight (rill-highlight.ts: Shiki's tokens, from Rust)
// runs in a Web Worker (highlight-worker.ts), so tokenizing never runs on the main
// thread; results come back through the plugin's own contract (a cached result now, or
// `null` and the callback later). Without workers, or if the worker fails,
// rill-highlight runs on the main thread; with any other code plugin, that plugin.
import type { PluginConfig } from "streamdown";

import {
  highlightCounts as mainCounts,
  setRillHighlightWasm,
  withRill,
  type Callback,
  type Options,
  type Result,
} from "./rill-highlight.ts";

type CodePlugin = NonNullable<PluginConfig["code"]>;

/** `plugins.code.highlight`'s contract, plus dropping a request nobody waits for. */
export type Highlighter = {
  highlight(options: Options, callback: Callback): Result | null;
  cancel(callback: Callback): void;
};

let enabled = true;
let wasmUrl = new URL("../wasm/rill_highlight.wasm", import.meta.url);

/** Whether rill-highlight replaces Shiki for the stock code plugin (default `true`). */
export function setRillHighlight(value: boolean) {
  enabled = value;
}

/** Where rill-highlight's WebAssembly is served from (default: the package's
 * `wasm/rill_highlight.wasm`, next to this module). Call before the first code block. */
export function setHighlightWasm(url: URL) {
  wasmUrl = url;
  setRillHighlightWasm(url);
}

const workerCounts = { rill: 0, shiki: 0 };

/** Results rill-highlight and Shiki returned, on the main thread and in the worker
 * (diagnostics, for tests and benchmarks). */
export function highlightCounts() {
  return {
    rill: mainCounts.rill + workerCounts.rill,
    shiki: mainCounts.shiki + workerCounts.shiki,
  };
}

/** Results of finished blocks, least recently used first. */
const MAX_CACHED = 500;

type Pending = { key: string; options: Options; callbacks: Callback[] };

let worker: Worker | null | undefined;
const pending = new Map<number, Pending>();
const cache = new Map<string, Result>();
let nextId = 0;
/** Languages the worker sent back to Shiki. */
const shikiLanguages = new Set<string>();
let stockPlugin: CodePlugin | undefined;
/** The rows last received per highlighter channel (see highlight-worker.ts). */
const received = new Map<string, Result["tokens"]>();
const onMainThread = new WeakMap<CodePlugin, Highlighter>();

function mainThread(plugin: CodePlugin): Highlighter {
  let highlighter = onMainThread.get(plugin);
  if (!highlighter) {
    const rill = withRill(plugin);
    highlighter = { highlight: rill.highlight, cancel: () => {} };
    onMainThread.set(plugin, highlighter);
  }
  return highlighter;
}

function startWorker(plugin: CodePlugin): Worker | null {
  if (worker !== undefined) return worker;
  try {
    worker = new Worker(new URL("./highlight-worker.js", import.meta.url), {
      type: "module",
    });
  } catch {
    worker = null;
    return null;
  }
  worker.postMessage({ type: "init", wasm: wasmUrl.href });
  worker.onmessage = ({
    data,
  }: MessageEvent<
    | { id: number; channel: string; result: Result; counts: typeof workerCounts }
    | { id: number; fallback: true }
  >) => {
    if ("fallback" in data) {
      // A grammar rill-highlight cannot compile: the page's own code plugin (Shiki).
      const request = pending.get(data.id);
      pending.delete(data.id);
      if (!request) return;
      shikiLanguages.add(request.options.language);
      for (const callback of request.callbacks) {
        const result = plugin.highlight(request.options, (late) => {
          workerCounts.shiki += 1;
          callback(late);
        });
        if (result) {
          workerCounts.shiki += 1;
          callback(result);
        }
      }
      return;
    }
    Object.assign(workerCounts, data.counts);
    // Rows the worker left out are the ones it sent at that index last time.
    const previous = received.get(data.channel) ?? [];
    const tokens = data.result.tokens.map((row, index) => row ?? previous[index] ?? []);
    received.set(data.channel, tokens);
    data.result.tokens = tokens;
    const request = pending.get(data.id);
    if (!request) return;
    pending.delete(data.id);
    if (!request.options.isIncomplete) {
      cache.delete(request.key);
      cache.set(request.key, data.result);
      if (cache.size > MAX_CACHED) cache.delete(cache.keys().next().value as string);
    }
    for (const callback of request.callbacks) callback(data.result);
  };
  worker.onerror = () => {
    // Highlight what was waiting on the main thread, and everything after it.
    worker?.terminate();
    worker = null;
    const highlighter = mainThread(plugin);
    for (const request of pending.values()) {
      for (const callback of request.callbacks) {
        const result = highlighter.highlight(request.options, callback);
        if (result) callback(result);
      }
    }
    pending.clear();
  };
  return worker;
}

const themeName = (theme: unknown) =>
  typeof theme === "string"
    ? theme
    : ((theme as { name?: string } | null)?.name ?? "custom");

const inWorker: Highlighter = {
  highlight(options, callback) {
    if (shikiLanguages.has(options.language) && stockPlugin) {
      return stockPlugin.highlight(options, callback);
    }
    const key = `${options.language}:${themeName(options.themes[0])}:${themeName(options.themes[1])}:${options.isIncomplete ? 1 : 0}\n${options.code}`;
    const cached = cache.get(key);
    if (cached) {
      cache.delete(key);
      cache.set(key, cached);
      return cached;
    }
    for (const request of pending.values()) {
      if (request.key === key) {
        request.callbacks.push(callback);
        return null;
      }
    }
    const id = (nextId += 1);
    pending.set(id, { key, options, callbacks: [callback] });
    worker?.postMessage({ type: "highlight", id, options });
    return null;
  },
  cancel(callback) {
    for (const [id, request] of pending) {
      const index = request.callbacks.indexOf(callback);
      if (index < 0) continue;
      request.callbacks.splice(index, 1);
      if (request.callbacks.length === 0) {
        pending.delete(id);
        worker?.postMessage({ type: "cancel", id });
      }
      return;
    }
  },
};

/** How many highlight requests the worker has not answered (for tests). */
export function pendingHighlights(): number {
  return pending.size;
}

const plain = new WeakMap<CodePlugin, Highlighter>();

/** The highlighter for `plugins.code`, or `undefined` without one. */
export function highlighterFor(plugin: CodePlugin | undefined): Highlighter | undefined {
  if (!plugin) return undefined;
  const stock = plugin.name === "shiki" && plugin.type === "code-highlighter";
  if (!(enabled && stock)) {
    let highlighter = plain.get(plugin);
    if (!highlighter) {
      highlighter = { highlight: (options, callback) => plugin.highlight(options, callback), cancel: () => {} };
      plain.set(plugin, highlighter);
    }
    return highlighter;
  }
  if (typeof Worker === "undefined" || typeof window === "undefined") {
    return mainThread(plugin);
  }
  stockPlugin = plugin;
  return startWorker(plugin) ? inWorker : mainThread(plugin);
}
