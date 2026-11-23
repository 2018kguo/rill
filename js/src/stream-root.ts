// The streaming root: Streamdown's root element with rill-stream keeping its children,
// for `<Streamdown>` props that leave Streamdown's default DOM as it is. Shared by the
// drop-in (`./streamdown`) and the light entry (`./stream`), which imports nothing of
// Streamdown's until a message needs it.
import { clsx } from "clsx";
import {
  createElement,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type MouseEvent,
  type ReactElement,
} from "react";
import { twMerge } from "tailwind-merge";

import { createStreamRenderer, type StreamRenderer } from "./stream-dom.ts";
import { loadStream, loadedStream, subscribeStream, type StreamOptions, type StreamSession } from "./stream.ts";

/** Streamdown's root element's classes. */
const ROOT_CLASS = "space-y-4 whitespace-normal [&>*:first-child]:mt-0 [&>*:last-child]:mb-0";

/** The props the streaming root renders exactly as Streamdown does (beside `data-*` and
 * `aria-*`): the rest change the DOM or need Streamdown's components. `linkSafety` and
 * `shikiTheme` only matter once a control is used (the message then hands off). */
const STREAM_PROPS = new Set([
  "children",
  "isAnimating",
  "mode",
  "className",
  "controls",
  "linkSafety",
  "shikiTheme",
  "parseIncompleteMarkdown",
  "normalizeHtmlIndentation",
  "lineNumbers",
  "codeBlockMaxHeight",
  "dir",
]);

type Props = Record<string, unknown>;

export function streamEligible(props: Props): boolean {
  if (typeof props.children !== "string" || props.mode === "static" || props.parseIncompleteMarkdown === false) {
    return false;
  }
  if (props.controls !== undefined && typeof props.controls !== "boolean") return false;
  if (props.dir !== undefined && props.dir !== "ltr" && props.dir !== "rtl") return false;
  const maxHeight = props.codeBlockMaxHeight;
  if (maxHeight !== undefined && !(typeof maxHeight === "number" && Number.isInteger(maxHeight) && maxHeight >= 0)) {
    return false;
  }
  for (const [name, value] of Object.entries(props)) {
    // (`animated={false}` is the default.)
    if (name === "animated" && value === false) continue;
    if (value !== undefined && !STREAM_PROPS.has(name) && !name.startsWith("data-") && !name.startsWith("aria-")) {
      return false;
    }
  }
  return true;
}

/** A click on the streaming root's button `button`, the root child `at` of `parent`. */
type Replay = { parent: Node; at: number; button: number };

/**
 * The streaming root for `props` while it can show them, else `null` (the caller renders
 * Streamdown's components). Once a message needs the exact path, or a button of it is
 * clicked, it stays with the caller; the click goes to the same button there.
 */
export function useStreamRoot(props: Props, waitForStream = false): ReactElement | null {
  const eligible = streamEligible(props);
  const stream = useSyncExternalStore(subscribeStream, loadedStream, () => null);
  useEffect(() => {
    if (eligible) void loadStream();
  }, [eligible]);
  const [handedOff, setHandedOff] = useState(false);
  const replay = useRef<Replay | null>(null);
  useLayoutEffect(() => {
    const pending = replay.current;
    replay.current = null;
    const root = pending?.parent.childNodes[pending.at] as Element | undefined;
    (root?.querySelectorAll("button")[pending!.button] as HTMLElement | undefined)?.click();
  });
  if (!eligible || handedOff) return null;
  const className = twMerge(clsx(ROOT_CLASS, props.className as string | undefined));
  if (stream === null) {
    // (The light entry shows the root empty until the module is in, rather than load
    // all of Streamdown for the first frames.)
    return waitForStream ? createElement("div", { className }) : null;
  }
  const maxHeight = props.codeBlockMaxHeight as number | undefined;
  const attributes: Props = {};
  for (const [name, value] of Object.entries(props)) {
    if (name.startsWith("data-") || name.startsWith("aria-")) attributes[name] = value;
  }
  return createElement(StreamRoot, {
    text: props.children as string,
    options: {
      controls: props.controls !== false,
      animating: Boolean(props.isAnimating),
      lineNumbers: props.lineNumbers !== false,
      codeMaxHeight: maxHeight === undefined ? 400 : maxHeight,
      dir: props.dir === "ltr" ? 1 : props.dir === "rtl" ? 2 : 0,
    },
    className,
    attributes,
    onHandOff: (click: Replay | null) => {
      replay.current = click;
      setHandedOff(true);
    },
  });
}

type StreamRootProps = {
  text: string;
  options: StreamOptions;
  className: string;
  attributes: Props;
  /** The text needs the exact path, or a button was clicked. */
  onHandOff(click: Replay | null): void;
};

/** Streamdown's root element, its children kept by rill-stream: each update renders only
 * the open tail and edits the DOM in place. */
function StreamRoot({ text, options, className, attributes, onHandOff }: StreamRootProps) {
  const root = useRef<HTMLDivElement>(null);
  const state = useRef<{ renderer: StreamRenderer; session: StreamSession; key: string; animating: boolean } | null>(null);
  // (`isAnimating` only changes the open tail and the controls' `disabled`.)
  const key = JSON.stringify({ ...options, animating: undefined });
  useLayoutEffect(() => {
    const module = loadedStream()!;
    let current = state.current;
    if (current?.key === key && current.animating && !options.animating) {
      // The message finished: the same session, its tail rendered again.
      current.session.setOptions(options.controls, false);
      current.animating = false;
      root.current!.querySelectorAll("button[disabled]").forEach((button) => button.removeAttribute("disabled"));
      if (!current.renderer.update(text)) onHandOff(null);
      return;
    }
    // Other options: the text renders again from the start.
    if (current?.key !== key || current.animating !== options.animating) {
      current?.renderer.dispose();
      root.current!.textContent = "";
      const session = module.newSession(options);
      const renderer = createStreamRenderer(root.current!, session, module);
      current = state.current = { renderer, session, key, animating: options.animating };
    }
    if (!current.renderer.update(text)) onHandOff(null);
  });
  useEffect(
    () => () => {
      state.current?.renderer.dispose();
      state.current = null;
    },
    []
  );
  return createElement("div", {
    ...attributes,
    ref: root,
    className,
    onClick: (event: MouseEvent<HTMLDivElement>) => {
      const element = root.current!;
      const button = (event.target as Element | null)?.closest?.("button");
      if (!button || !element.contains(button)) return;
      event.preventDefault();
      event.stopPropagation();
      const parent = element.parentNode!;
      onHandOff({
        parent,
        at: Array.prototype.indexOf.call(parent.childNodes, element),
        button: Array.prototype.indexOf.call(element.querySelectorAll("button"), button),
      });
    },
  });
}
