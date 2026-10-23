// rill-highlight in a Web Worker, for the drop-in's code blocks (see highlight.ts):
// tokenizing never runs on the page's main thread. A request rill-highlight cannot take
// (a grammar that does not compile) is answered with `fallback`, and the page asks the
// code plugin it was given (Shiki). Requests cancelled before their turn are skipped.
import type { Options, Result } from "./rill-highlight.ts";
import { highlightCounts, preloadRill, setRillHighlightWasm, withRill } from "./rill-highlight.ts";

type Message =
  | { type: "init"; wasm: string | null }
  | { type: "highlight"; id: number; options: Options }
  | { type: "cancel"; id: number };

const scope = globalThis as unknown as {
  postMessage(message: unknown): void;
  onmessage: ((event: { data: Message }) => void) | null;
};

// rill-highlight's fallback, answered with a marker the reply passes to the page.
const FALLBACK = { fallback: true } as unknown as Result;
const highlighter = withRill({
  highlight(_options: Options, callback: (result: Result) => void) {
    callback(FALLBACK);
    return null;
  },
});

/** The rows last sent per highlighter (language and themes). Finished lines keep their
 * rows from request to request, so a reply sends only rows that are not the ones sent
 * at the same index last time (`null` for those), and the page rebuilds the rest. */
const sent = new Map<string, Result["tokens"]>();
const queue: { id: number; options: Options }[] = [];
let scheduled = false;
/** Replies not posted yet: one message carries all a task produced, so the page
 * applies them in one task (one React render instead of one per code block). */
let outbox: unknown[] = [];
let flushing = false;

function post(reply: unknown) {
  outbox.push(reply);
  if (flushing) return;
  flushing = true;
  queueMicrotask(() => {
    flushing = false;
    const batch = outbox;
    outbox = [];
    scope.postMessage({ batch });
  });
}

/** Answers every queued request (results for languages still loading come later). */
function run() {
  scheduled = false;
  for (let request = queue.shift(); request; request = queue.shift()) {
    const { id, options } = request;
    const channel = `${options.language}\0${JSON.stringify(options.themes)}`;
    const reply = (result: Result) => {
      if (result === FALLBACK) {
        post({ id, fallback: true });
        return;
      }
      const previous = sent.get(channel) ?? [];
      const rows = result.tokens.map((row, index) => (previous[index] === row ? null : row));
      sent.set(channel, result.tokens);
      // Shiki's grammar state stays here; the page never reads it.
      const { grammarState: _state, ...rest } = result as Result & { grammarState?: unknown };
      post({ id, channel, result: { ...rest, tokens: rows }, counts: highlightCounts });
    };
    const cached = highlighter.highlight(options, reply);
    if (cached) reply(cached);
  }
}

function schedule() {
  if (scheduled || queue.length === 0) return;
  scheduled = true;
  // Let cancellations that arrived meanwhile land first.
  setTimeout(run, 0);
}

scope.onmessage = ({ data }) => {
  if (data.type === "init") {
    if (data.wasm) setRillHighlightWasm(new URL(data.wasm));
    // Load the WebAssembly and Streamdown's default themes before the first request.
    preloadRill(["github-light", "github-dark"]);
  } else if (data.type === "cancel") {
    const index = queue.findIndex((request) => request.id === data.id);
    if (index >= 0) queue.splice(index, 1);
  } else {
    queue.push({ id: data.id, options: data.options });
    schedule();
  }
};
