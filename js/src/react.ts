// The drop-in: `<Streamdown {...useRill()}>`. Until the WebAssembly arrives (and
// during SSR) the hook returns `null` and Streamdown renders with its own plugins; the
// trees are the same, so the switch is invisible.
import { useEffect, useMemo, useSyncExternalStore } from "react";

import { loadRill, type Rill } from "./index.ts";
import { rillPlugins } from "./streamdown.ts";

let rill: Rill | null = null;
const listeners = new Set<() => void>();

function load() {
  if (rill) {
    return;
  }
  loadRill()
    .then((loaded) => {
      rill = loaded;
      for (const listener of listeners) {
        listener();
      }
    })
    // Offline or deploy skew: Streamdown keeps rendering; the next mount retries.
    .catch(() => {});
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Streamdown's plugins on rill once it loads, else `null` (spread it either way). */
export function useRill() {
  const current = useSyncExternalStore(
    subscribe,
    () => rill,
    () => null
  );
  useEffect(load, []);
  return useMemo(() => (current ? rillPlugins({ rill: current }) : null), [current]);
}
