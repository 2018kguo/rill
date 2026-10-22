// The drop-in: `<Streamdown {...useRill()}>`. Until the WebAssembly arrives (and
// during SSR) the hook returns `null` and Streamdown renders with its own plugins; the
// trees are the same, so the switch is invisible.
import { useEffect, useSyncExternalStore } from "react";

import { loadRill } from "./index.ts";
import { rillPlugins } from "./streamdown.ts";

type Plugins = ReturnType<typeof rillPlugins>;

// One set per page, shared by every message (and Streamdown's processor cache).
let plugins: Plugins | null = null;
let attempted = false;
const listeners = new Set<() => void>();

function load() {
  // One attempt per page: a failure (no WebAssembly, a missing asset) would repeat.
  if (attempted) {
    return;
  }
  attempted = true;
  loadRill()
    .then((rill) => {
      plugins = rillPlugins({ rill });
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

/** Streamdown's plugins on rill once it loads, else `null` (spread it either way). */
export function useRill(): Plugins | null {
  const current = useSyncExternalStore(
    subscribe,
    () => plugins,
    () => null
  );
  useEffect(load, []);
  return current;
}
