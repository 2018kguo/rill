// rill-stream: the streaming renderer for messages in Streamdown's default setup. Its
// own small WebAssembly build renders each update's open tail into DOM edits; React
// owns only the root element.
import { type StreamModule, type StreamSession as Session } from "./stream-dom.ts";

export { createStreamRenderer, type StreamRenderer } from "./stream-dom.ts";

type StreamGlue = {
  default(options: { module_or_path: unknown }): Promise<unknown>;
  initSync(options: { module: BufferSource | WebAssembly.Module }): unknown;
  Stream: new () => StreamSession & {
    setLayout(lineNumbers: boolean, codeMaxHeight: number, dir: number): void;
    setOverrides(overrides: number): void;
  };
  streamMemory(): WebAssembly.Memory;
  streamTags(): string;
};

export type StreamSession = Session & { setOptions(controls: boolean, animating: boolean): void };

/** Streamdown's props the streaming DOM depends on. */
export type StreamOptions = {
  controls: boolean;
  animating: boolean;
  lineNumbers: boolean;
  /** `codeBlockMaxHeight` in pixels (0: none). */
  codeMaxHeight: number;
  /** 0: no `dir`, 1: `ltr`, 2: `rtl`. */
  dir: number;
  /** The tags the page's `components` override (`OVERRIDES` bits): islands. */
  overrides: number;
};

/** The `components` the stream renders as islands, and each one's bit. */
export const OVERRIDES: Record<string, number> = { a: 1, table: 2, span: 4 };

export type LoadedStream = StreamModule & { newSession(options: StreamOptions): StreamSession };

let loaded: LoadedStream | null = null;
let loading: Promise<void> | undefined;
const listeners = new Set<() => void>();
let version = 0;
const changed = () => {
  version += 1;
  for (const listener of listeners) listener();
};

/** Bumped as the streaming build or the class merge loads. */
export const streamVersion = (): number => version;

type Merge = (...classes: unknown[]) => string;
let merge: Merge | null = null;
let merging: Promise<void> | undefined;

type Islands = typeof import("./stream-islands.ts");
let islands: Islands | null = null;
let loadingIslands: Promise<void> | undefined;

/** The islands' renderer (for `components`) once loaded, else `null`. */
export const loadedIslands = (): Islands | null => islands;

export function loadIslands(): Promise<void> {
  loadingIslands ??= import("./stream-islands.ts")
    .then((module) => {
      islands = module;
      changed();
    })
    .catch(() => {
      loadingIslands = undefined;
    });
  return loadingIslands;
}

/** Streamdown's class merge (`twMerge(clsx(...))`) once loaded, else `null`. */
export const loadedMerge = (): Merge | null => merge;

export function loadMerge(): Promise<void> {
  merging ??= Promise.all([import("clsx"), import("tailwind-merge")])
    .then(([{ clsx }, { twMerge }]) => {
      merge = (...classes) => twMerge(clsx(...(classes as Parameters<typeof clsx>)));
      changed();
    })
    .catch(() => {
      merging = undefined;
    });
  return merging;
}

const ready = (glue: StreamGlue) => {
  loaded = {
    memory: glue.streamMemory(),
    tags: glue.streamTags(),
    newSession(options) {
      const session = new glue.Stream();
      session.setOptions(options.controls, options.animating);
      session.setLayout(options.lineNumbers, options.codeMaxHeight, options.dir);
      session.setOverrides(options.overrides);
      return session;
    },
  };
  changed();
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
      await glue.default({
        module_or_path: new URL("../wasm/rill_stream_bg.wasm", import.meta.url),
      });
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
