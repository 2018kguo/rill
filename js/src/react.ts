// The drop-in: `<Streamdown plugins={plugins} {...useRill(plugins)}>`. Until the
// WebAssembly arrives (and during SSR) the hook returns `null` and Streamdown renders
// with its own plugins; the trees are the same, so the switch is invisible.
import { useEffect, useSyncExternalStore } from "react";
import type { PluginConfig } from "streamdown";

import type { Rill } from "./core.ts";
import { loadRill } from "./index.ts";
import { rillPlugins } from "./streamdown.ts";

type Plugins = ReturnType<typeof rillPlugins>;
type StreamdownPlugins = Pick<PluginConfig, "math" | "cjk">;

// One set per `plugins` object, shared by every message (and Streamdown's processor
// cache).
let rill: Rill | null = null;
let withoutPlugins: Plugins | undefined;
const byPlugins = new WeakMap<StreamdownPlugins, Plugins>();
let attempted = false;
const listeners = new Set<() => void>();

function load() {
  // One attempt per page: a failure (no WebAssembly, a missing asset) would repeat.
  if (attempted) {
    return;
  }
  attempted = true;
  loadRill()
    .then((loaded) => {
      rill = loaded;
      for (const listener of listeners) {
        listener();
      }
    })
    .catch((error: unknown) => {
      // Streamdown keeps rendering, with the same output.
      console.warn(
        "rill: the WebAssembly did not load, so Streamdown renders in JavaScript. " +
          "With Vite, add `optimizeDeps: { exclude: ['@2018kguo/rill'] }`.",
        error
      );
    });
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function pluginsFor(loaded: Rill, plugins: StreamdownPlugins | undefined) {
  if (plugins === undefined) {
    withoutPlugins ??= rillPlugins({ rill: loaded });
    return withoutPlugins;
  }
  let set = byPlugins.get(plugins);
  if (set === undefined) {
    set = rillPlugins({ rill: loaded, plugins });
    byPlugins.set(plugins, set);
  }
  return set;
}

/**
 * Streamdown's plugins on rill once it loads, else `null` (spread it either way). Pass
 * the `plugins` you give `<Streamdown>` (a stable object): math and CJK then render in
 * rill too.
 */
export function useRill(plugins?: StreamdownPlugins): Plugins | null {
  const loaded = useSyncExternalStore(
    subscribe,
    () => rill,
    () => null
  );
  useEffect(load, []);
  return loaded === null ? null : pluginsFor(loaded, plugins);
}
