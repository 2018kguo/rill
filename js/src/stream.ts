// rill-stream: the streaming renderer for messages in Streamdown's default setup. Its
// own small WebAssembly build renders each update's open tail into DOM edits; React
// owns only the root element.
import { createStreamRenderer, type StreamModule, type StreamSession } from "./stream-dom.ts";

export { createStreamRenderer, type StreamRenderer } from "./stream-dom.ts";

type StreamGlue = {
  default(options: { module_or_path: unknown }): Promise<unknown>;
  initSync(options: { module: BufferSource | WebAssembly.Module }): unknown;
  Stream: new () => StreamSession & {
    setOptions(controls: boolean, animating: boolean): void;
    setLayout(lineNumbers: boolean, codeMaxHeight: number, dir: number): void;
  };
  streamMemory(): WebAssembly.Memory;
  streamTags(): string;
};

/** Streamdown's props the streaming DOM depends on. */
export type StreamOptions = {
  controls: boolean;
  animating: boolean;
  lineNumbers: boolean;
  /** `codeBlockMaxHeight` in pixels (0: none). */
  codeMaxHeight: number;
  /** 0: no `dir`, 1: `ltr`, 2: `rtl`. */
  dir: number;
};

export type LoadedStream = StreamModule & { newSession(options: StreamOptions): StreamSession };

let loaded: LoadedStream | null = null;
let loading: Promise<void> | undefined;
const listeners = new Set<() => void>();

const ready = (glue: StreamGlue) => {
  loaded = {
    memory: glue.streamMemory(),
    tags: glue.streamTags(),
    newSession(options) {
      const session = new glue.Stream();
      session.setOptions(options.controls, options.animating);
      session.setLayout(options.lineNumbers, options.codeMaxHeight, options.dir);
      return session;
    },
  };
  for (const listener of listeners) listener();
};

/** The streaming build once loaded, else `null`. */
export const loadedStream = (): LoadedStream | null => loaded;

export function subscribeStream(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Fetches the streaming build once (a failed fetch is tried again next time). */
export function loadStream(): Promise<void> {
  loading ??= import("../wasm/rill_stream.js")
    .then(async (glue: StreamGlue) => {
      await glue.default({ module_or_path: new URL("../wasm/rill_stream_bg.wasm", import.meta.url) });
      ready(glue);
    })
    .catch(() => {
      loading = undefined;
    });
  return loading;
}

/** Instantiates the streaming build from its bytes (Node, SSR, tests). */
export async function initStream(wasm: BufferSource | WebAssembly.Module): Promise<void> {
  const glue = (await import("../wasm/rill_stream.js")) as StreamGlue;
  glue.initSync({ module: wasm });
  ready(glue);
}
