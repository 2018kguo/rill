// `@2018kguo/rill/stream`: `<Streamdown>` for pages that care about download size. A
// message in Streamdown's default setup streams through rill-stream alone; anything
// else loads the full drop-in (and Streamdown) when it first needs it.
import { createElement, lazy, memo, Suspense, type ComponentType } from "react";

import { useStreamRoot } from "./stream-root.ts";

const Full = lazy(async () => ({ default: (await import("./streamdown.ts")).Streamdown as ComponentType<Record<string, unknown>> }));

export const Streamdown = memo(function Streamdown(props: Record<string, unknown>) {
  return useStreamRoot(props, true) ?? createElement(Suspense, { fallback: null }, createElement(Full, props));
}) as unknown as typeof import("./streamdown.ts").Streamdown;

export { initStream, loadStream } from "./stream.ts";
