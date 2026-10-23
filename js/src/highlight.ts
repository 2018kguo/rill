// Highlighting for the drop-in's code blocks. With Streamdown's stock code plugin
// (`@streamdown/code`), Shiki runs in a Web Worker (highlight-worker.ts) that runs the
// same package, so the tokens are the same; results come back through the plugin's own
// contract (a cached result now, or `null` and the callback later). Anywhere a worker
// cannot run, or when it fails, the plugin highlights on the main thread as before.
import type { PluginConfig } from "streamdown";

type CodePlugin = NonNullable<PluginConfig["code"]>;
type Options = Parameters<CodePlugin["highlight"]>[0];
type Result = NonNullable<ReturnType<CodePlugin["highlight"]>>;
type Callback = (result: Result) => void;

/** `plugins.code.highlight`'s contract, plus dropping a request nobody waits for. */
export type Highlighter = {
  highlight(options: Options, callback: Callback): Result | null;
  cancel(callback: Callback): void;
};

let enabled = true;

/** Whether the stock code plugin highlights in a Web Worker (default `true`). */
export function setHighlightWorker(value: boolean) {
  enabled = value;
}

const onMainThread = new WeakMap<CodePlugin, Highlighter>();

function mainThread(plugin: CodePlugin): Highlighter {
  let highlighter = onMainThread.get(plugin);
  if (!highlighter) {
    highlighter = {
      highlight: (options, callback) => plugin.highlight(options, callback),
      cancel: () => {},
    };
    onMainThread.set(plugin, highlighter);
  }
  return highlighter;
}

/** Results of finished blocks, least recently used first (as the plugin keeps them). */
const MAX_CACHED = 500;

type Pending = { key: string; options: Options; callbacks: Callback[] };

let worker: Worker | null | undefined;
const pending = new Map<number, Pending>();
const cache = new Map<string, Result>();
let nextId = 0;
/** The rows last received per highlighter channel (see highlight-worker.ts). */
const received = new Map<string, Result["tokens"]>();
let failedPlugin: CodePlugin | undefined;

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
  worker.onmessage = ({
    data,
  }: MessageEvent<{ id: number; channel: string; result: Result }>) => {
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
    failedPlugin = plugin;
    for (const request of pending.values()) {
      for (const callback of request.callbacks) {
        const result = plugin.highlight(request.options, callback);
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

/** The highlighter for `plugins.code`, or `undefined` without one. */
export function highlighterFor(plugin: CodePlugin | undefined): Highlighter | undefined {
  if (!plugin) return undefined;
  const stock = plugin.name === "shiki" && plugin.type === "code-highlighter";
  if (
    !(enabled && stock) ||
    failedPlugin !== undefined ||
    typeof Worker === "undefined" ||
    typeof window === "undefined"
  ) {
    return mainThread(plugin);
  }
  return startWorker(plugin) ? inWorker : mainThread(plugin);
}
