// Highlighting for the drop-in's code blocks. With Streamdown's stock code plugin
// (`@streamdown/code`), rill-highlight (rill-highlight.ts: Shiki's tokens, from Rust)
// runs in a Web Worker (highlight-worker.ts), so tokenizing never runs on the main
// thread; results come back through the plugin's own contract (a cached result now, or
// `null` and the callback later). Without workers, or if the worker fails,
// rill-highlight runs on the main thread; with any other code plugin, that plugin.
import type { PluginConfig } from "streamdown";

import { Lru } from "./prefix.ts";
import type { Callback, Options, Result } from "./rill-highlight.ts";

// rill-highlight on the main thread (no workers: the server, tests, old browsers) is
// loaded when first needed, so the page's bundle does not carry it.
type RillModule = typeof import("./rill-highlight.ts");
let rillModule: RillModule | undefined;
let loadingModule: Promise<RillModule> | undefined;
const loadRillModule = () =>
  (loadingModule ??= import("./rill-highlight.ts").then((module) => {
    if (wasmUrl) module.setRillHighlightWasm(wasmUrl);
    rillModule = module;
    return module;
  }));

type CodePlugin = NonNullable<PluginConfig["code"]>;

/** `plugins.code.highlight`'s contract, plus dropping a request nobody waits for. */
export type Highlighter = {
  highlight(options: Options, callback: Callback): Result | null;
  cancel(callback: Callback): void;
};

let enabled = true;
/** Where rill-highlight's WebAssembly is served, when the page says (else its package
 * finds it). */
let wasmUrl: URL | undefined;

/** Whether rill-highlight replaces Shiki for the stock code plugin (default `true`). */
export function setRillHighlight(value: boolean) {
  enabled = value;
}

/** Where rill-highlight's WebAssembly is served from (default: @2018kguo/rill-highlight's
 * `dist/rill_highlight.wasm`, where bundlers that rewrite `new URL(..., import.meta.url)`
 * put it; with others, serve that file and pass its URL). Call before the first code
 * block: the highlighting worker starts then, and loads it and the default themes. */
export function setHighlightWasm(url: URL) {
  wasmUrl = url;
  rillModule?.setRillHighlightWasm(url);
  preloadHighlighting();
}

const workerCounts = { rill: 0, shiki: 0 };

/** Results rill-highlight and Shiki returned, on the main thread and in the worker
 * (diagnostics, for tests and benchmarks). */
export function highlightCounts() {
  return {
    rill: (rillModule?.highlightCounts.rill ?? 0) + workerCounts.rill,
    shiki: (rillModule?.highlightCounts.shiki ?? 0) + workerCounts.shiki,
  };
}

/** Results of finished blocks, least recently used first. */
const MAX_CACHED = 500;

type Pending = { key: string; options: Options; callbacks: Callback[] };

let worker: Worker | null | undefined;
const pending = new Map<number, Pending>();
/** The pending request for each key (a request already sent is joined, not resent). */
const pendingByKey = new Map<string, number>();
/** The request each callback waits on. */
const waiting = new WeakMap<Callback, number>();

function forget(id: number, request: Pending) {
  pending.delete(id);
  if (pendingByKey.get(request.key) === id) pendingByKey.delete(request.key);
}
// Weighed by the code (its tokens run some times its length): a code block streaming
// without isAnimating asks for every prefix, and 500 of those can be large.
const cache = new Lru<string, Result>(MAX_CACHED, 8 << 20, (key) => 24 * key.length);
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
    let rill: ReturnType<RillModule["withRill"]> | undefined;
    highlighter = {
      highlight(options, callback) {
        if (!rill && rillModule) rill = rillModule.withRill(plugin);
        if (rill) return rill.highlight(options, callback);
        loadRillModule().then(
          (module) => {
            rill ??= module.withRill(plugin);
            const result = rill.highlight(options, callback);
            if (result) callback(result);
          },
          () => {
            const result = plugin.highlight(options, callback);
            if (result) callback(result);
          }
        );
        return null;
      },
      cancel: () => {},
    };
    onMainThread.set(plugin, highlighter);
  }
  return highlighter;
}

/** The worker, started now if it has not been (its WebAssembly and default themes load
 * at once, before any request). Its replies go to the stock plugin `highlighterFor`
 * was given, which every request comes with. */
function startWorker(): Worker | null {
  if (worker !== undefined) return worker;
  try {
    worker = new Worker(new URL("./highlight-worker.js", import.meta.url), {
      type: "module",
    });
  } catch {
    worker = null;
    return null;
  }
  worker.postMessage({ type: "init", wasm: wasmUrl?.href ?? null });
  type Reply =
    | { id: number; channel: string; result: Result; counts: typeof workerCounts }
    | { id: number; fallback: true };
  const handle = (data: Reply) => {
    if ("fallback" in data) {
      // A grammar rill-highlight cannot compile: the page's own code plugin (Shiki).
      const request = pending.get(data.id);
      if (!request) return;
      forget(data.id, request);
      shikiLanguages.add(request.options.language);
      const plugin = stockPlugin!;
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
    forget(data.id, request);
    if (!request.options.isIncomplete) {
      cache.set(request.key, data.result);
    }
    for (const callback of request.callbacks) callback(data.result);
  };
  // One message holds every reply the worker produced in a task: their callbacks run in
  // this one task, so React renders them together.
  worker.onmessage = ({ data }: MessageEvent<{ batch: Reply[] }>) => {
    for (const reply of data.batch) handle(reply);
  };
  worker.onerror = () => {
    // Highlight what was waiting on the main thread, and everything after it.
    worker?.terminate();
    worker = null;
    if (!stockPlugin) return;
    const highlighter = mainThread(stockPlugin);
    for (const request of pending.values()) {
      for (const callback of request.callbacks) {
        const result = highlighter.highlight(request.options, callback);
        if (result) callback(result);
      }
    }
    pending.clear();
    pendingByKey.clear();
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
    if (cached) return cached;
    const joined = pendingByKey.get(key);
    if (joined !== undefined) {
      pending.get(joined)!.callbacks.push(callback);
      waiting.set(callback, joined);
      return null;
    }
    const id = (nextId += 1);
    pending.set(id, { key, options, callbacks: [callback] });
    pendingByKey.set(key, id);
    waiting.set(callback, id);
    worker?.postMessage({ type: "highlight", id, options });
    return null;
  },
  cancel(callback) {
    const id = waiting.get(callback);
    const request = id === undefined ? undefined : pending.get(id);
    if (id === undefined || !request) return;
    waiting.delete(callback);
    const index = request.callbacks.indexOf(callback);
    if (index < 0) return;
    request.callbacks.splice(index, 1);
    if (request.callbacks.length === 0) {
      forget(id, request);
      worker?.postMessage({ type: "cancel", id });
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
  return startWorker() ? inWorker : mainThread(plugin);
}

/** Starts the highlighting worker (with its WebAssembly and themes) ahead of the first
 * code block. `setHighlightWasm` calls it; without that, the first code block does. */
export function preloadHighlighting() {
  if (enabled && typeof Worker !== "undefined" && typeof window !== "undefined") startWorker();
}
