// The plugins tier: `<Streamdown plugins={plugins} {...useRill(plugins)}>`. Until the
// WebAssembly arrives (and during SSR) the hook returns `null` and Streamdown renders
// with its own plugins; the trees are the same, so the switch is invisible.
import { useEffect, useSyncExternalStore } from "react";
import type { PluginConfig } from "streamdown";

import type { Rill } from "./core.ts";
import type { Engine } from "./engine.ts";
import { rillEngine } from "./index.ts";
import { rillPlugins } from "./plugins.ts";

type Plugins = ReturnType<typeof rillPlugins>;
type StreamdownPlugins = Pick<PluginConfig, "math" | "cjk">;

// One set per rill and `plugins` object, shared by every message (and Streamdown's
// processor cache).
const sets = new WeakMap<Rill, WeakMap<object, Plugins>>();
const NO_PLUGINS = {};
const attempted = new WeakSet<object>();
const failed = new WeakSet<object>();

/** Whether `engine`'s WebAssembly failed to load. */
export const loadFailed = (engine: object): boolean => failed.has(engine);

/** Starts loading `engine`'s WebAssembly, once per page: a failure (no WebAssembly, a
 * missing asset) would only repeat. */
export function load(engine: Pick<Engine, "load">) {
  if (attempted.has(engine)) return;
  attempted.add(engine);
  engine.load().catch((error: unknown) => {
    failed.add(engine);
    // Streamdown keeps rendering, with the same output.
    console.warn(
      "rill: the WebAssembly did not load, so Streamdown renders in JavaScript. " +
        "With Vite, add `optimizeDeps: { exclude: ['@2018kguo/rill'] }`.",
      error
    );
  });
}

const subscribe = (listener: () => void) => rillEngine.subscribe(listener);
const snapshot = () => rillEngine.rill();

export function pluginsFor(loaded: Rill, plugins: StreamdownPlugins | undefined) {
  let byPlugins = sets.get(loaded);
  if (!byPlugins) {
    byPlugins = new WeakMap();
    sets.set(loaded, byPlugins);
  }
  let set = byPlugins.get(plugins ?? NO_PLUGINS);
  if (set === undefined) {
    set = rillPlugins(plugins ? { rill: loaded, plugins } : { rill: loaded });
    byPlugins.set(plugins ?? NO_PLUGINS, set);
  }
  return set;
}

/**
 * Streamdown's plugins on rill once it loads, else `null` (spread it either way). Pass
 * the `plugins` you give `<Streamdown>` (a stable object): math and CJK then render in
 * rill too.
 */
export function useRill(plugins?: StreamdownPlugins): Plugins | null {
  const loaded = useSyncExternalStore(subscribe, snapshot, () => null);
  useEffect(() => load(rillEngine), []);
  return loaded === null ? null : pluginsFor(loaded, plugins);
}
