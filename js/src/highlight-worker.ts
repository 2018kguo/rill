// The code plugin's highlighter (`@streamdown/code`, Shiki with its JavaScript regex
// engine) in a Web Worker: the same package, so the same tokens, off the main thread.
// Requests that are cancelled before their turn are skipped.
import { code } from "@streamdown/code";

type Request = { type: "highlight"; id: number; options: Parameters<typeof code.highlight>[0] };
type Message = Request | { type: "cancel"; id: number };

const queue: Request[] = [];
let scheduled = false;

const scope = globalThis as unknown as {
  postMessage(message: unknown): void;
  onmessage: ((event: { data: Message }) => void) | null;
};

function run() {
  scheduled = false;
  const request = queue.shift();
  if (!request) return;
  const reply = (result: unknown) => scope.postMessage({ id: request.id, result });
  const cached = code.highlight(request.options, reply);
  if (cached) reply(cached);
  schedule();
}

function schedule() {
  if (scheduled || queue.length === 0) return;
  scheduled = true;
  // Let cancellations that arrived meanwhile land first.
  setTimeout(run, 0);
}

scope.onmessage = ({ data }) => {
  if (data.type === "cancel") {
    const index = queue.findIndex((request) => request.id === data.id);
    if (index >= 0) queue.splice(index, 1);
    return;
  }
  queue.push(data);
  schedule();
};
