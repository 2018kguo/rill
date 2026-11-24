// The streaming root: Streamdown's root element with rill-stream keeping its children,
// for `<Streamdown>` props that leave Streamdown's default DOM as it is. Shared by the
// drop-in (`./streamdown`) and the light entry (`./stream`), which imports nothing of
// Streamdown's until a message needs it.
import {
  Fragment,
  createElement,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type MouseEvent,
  type ReactElement,
} from "react";
import type { PluginConfig } from "streamdown";

import type { Args } from "./core.ts";
import type { Engine } from "./engine.ts";

import {
  createStreamRenderer,
  isIslandNode,
  type Island,
  type StreamRenderer,
} from "./stream-dom.ts";
import {
  OVERRIDES,
  loadIslands,
  loadedIslands,
  loadMerge,
  loadStream,
  loadedMerge,
  loadedStream,
  streamVersion,
  subscribeStream,
  type LoadedStream,
  type StreamOptions,
  type StreamSession,
} from "./stream.ts";

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
  "components",
  "plugins",
  "mermaid",
]);

/** What a page's engine with a streaming build may pass besides (see `streamEligible`). */
const ENGINE_PROPS = new Set(["engine", "engineArgs", "BlockComponent", "parseMarkdownIntoBlocksFn"]);

/** The `plugins` the stream renders: `code` (code blocks become islands), and Mermaid
 * and `renderers`, whose blocks hand the message to Streamdown's components. */
const STREAM_PLUGINS = new Set(["code", "mermaid", "renderers"]);

/** The tags `components` overrides that stream as islands (bits of `OVERRIDES`), or
 * -1 when it overrides one that does not. */
function overridesOf(components: unknown): number {
  if (components === undefined) return 0;
  if (typeof components !== "object" || components === null) return -1;
  let mask = 0;
  for (const [tag, component] of Object.entries(components)) {
    if (component === undefined) continue;
    const bit = OVERRIDES[tag];
    if (bit === undefined) return -1;
    mask |= bit;
  }
  return mask;
}

type Props = Record<string, unknown>;

export function streamEligible(props: Props): boolean {
  if (
    typeof props.children !== "string" ||
    props.mode === "static" ||
    props.parseIncompleteMarkdown === false
  ) {
    return false;
  }
  if (props.controls !== undefined && typeof props.controls !== "boolean") return false;
  if (overridesOf(props.components) < 0) return false;
  // A page's engine with a streaming build: its blocks only add context (see
  // `EngineDefinition.stream`).
  const engine = props.engine as Engine<Args> | undefined;
  const own = engine !== undefined;
  if (own && !engine.stream) return false;
  const plugins = props.plugins as Record<string, unknown> | undefined;
  if (plugins !== undefined && Object.entries(plugins).some(([name, value]) => value !== undefined && !STREAM_PLUGINS.has(name))) {
    return false;
  }
  if (props.dir !== undefined && props.dir !== "ltr" && props.dir !== "rtl") return false;
  const maxHeight = props.codeBlockMaxHeight;
  if (
    maxHeight !== undefined &&
    !(typeof maxHeight === "number" && Number.isInteger(maxHeight) && maxHeight >= 0)
  ) {
    return false;
  }
  for (const [name, value] of Object.entries(props)) {
    // (`animated={false}` is the default.)
    if (name === "animated" && value === false) continue;
    if (
      value !== undefined &&
      !STREAM_PROPS.has(name) &&
      !(own && ENGINE_PROPS.has(name)) &&
      !name.startsWith("data-") &&
      !name.startsWith("aria-")
    ) {
      return false;
    }
  }
  return true;
}

const EMPTY = {};
const noSubscribe = () => () => {};
const builds = new WeakMap<object, number>();
let buildCount = 0;
const buildId = (stream: object): number => {
  if (!builds.has(stream)) builds.set(stream, (buildCount += 1));
  return builds.get(stream)!;
};
/** The `overrides` bit for code blocks as islands (rill-stream's `overrides::CODE`). */
const CODE_ISLANDS = 8;

/** A click on the streaming root's button `button`, the root child `at` of `parent`. */
type Replay = { parent: Node; at: number; button: number };

/**
 * The streaming root for `props` while it can show them, else `null` (the caller renders
 * Streamdown's components). Once a message needs the exact path, or a button of it is
 * clicked, it stays with the caller; the click goes to the same button there.
 */
export function useStreamRoot(props: Props, waitForStream = false): ReactElement | null {
  const eligible = streamEligible(props);
  // (A version, so the merge loading re-renders too; 0 on the server and while
  // hydrating, so the first client render matches the server's.)
  const version = useSyncExternalStore(subscribeStream, streamVersion, () => 0);
  const engine = props.engine as Engine<Args> | undefined;
  const args = (props.engineArgs ?? []) as Args;
  // The page's engine's build (its readiness is the engine's to announce).
  const engineVersion = useSyncExternalStore(
    engine?.subscribe ?? noSubscribe,
    () => (engine?.stream?.(...args) ? 1 : 0),
    () => 0
  );
  const stream = version === 0 && engineVersion === 0 ? null : engine ? (engine.stream?.(...args) ?? null) : loadedStream();
  const custom = props.className !== undefined;
  const plugins = props.plugins as PluginConfig | undefined;
  const overrides = eligible ? overridesOf(props.components) | (plugins?.code ? CODE_ISLANDS : 0) : 0;
  useEffect(() => {
    if (!eligible) return;
    if (engine) void engine.load();
    else void loadStream();
    if (custom) void loadMerge();
    if (overrides > 0) void loadIslands();
  }, [eligible, custom, overrides, engine]);
  const [handedOff, setHandedOff] = useState(false);
  const replay = useRef<Replay | null>(null);
  useLayoutEffect(() => {
    const pending = replay.current;
    replay.current = null;
    const root = pending?.parent.childNodes[pending.at] as Element | undefined;
    (root?.querySelectorAll("button")[pending!.button] as HTMLElement | undefined)?.click();
  });
  if (!eligible || handedOff) return null;
  // (Streamdown merges `className` into its root's with tailwind-merge, loaded only for a
  // message that passes one.)
  const merge = version === 0 ? null : loadedMerge();
  const className = !custom
    ? ROOT_CLASS
    : merge === null
      ? null
      : merge(ROOT_CLASS, props.className);
  const islands = version === 0 ? null : loadedIslands();
  if (stream === null || className === null || (overrides > 0 && islands === null)) {
    // (The light entry shows the root empty until the module is in, rather than load
    // all of Streamdown for the first frames.)
    return waitForStream ? createElement("div", { className: className ?? ROOT_CLASS }) : null;
  }
  const maxHeight = props.codeBlockMaxHeight as number | undefined;
  const attributes: Props = {};
  for (const [name, value] of Object.entries(props)) {
    if (name.startsWith("data-") || name.startsWith("aria-")) attributes[name] = value;
  }
  return createElement(StreamRoot, {
    stream,
    text: props.children as string,
    options: {
      controls: props.controls !== false,
      animating: Boolean(props.isAnimating),
      lineNumbers: props.lineNumbers !== false,
      codeMaxHeight: maxHeight === undefined ? 400 : maxHeight,
      dir: props.dir === "ltr" ? 1 : props.dir === "rtl" ? 2 : 0,
      overrides,
    },
    components: props.components as Record<string, unknown> | undefined,
    plugins,
    context: {
      // Streamdown's own context, as its root makes it.
      codeBlockMaxHeight: maxHeight ?? 400,
      shikiTheme: props.shikiTheme ?? plugins?.code?.getThemes() ?? ["github-light", "github-dark"],
      controls: props.controls ?? true,
      isAnimating: Boolean(props.isAnimating),
      lineNumbers: props.lineNumbers ?? true,
      mode: "streaming",
      mermaid: props.mermaid,
      linkSafety: props.linkSafety ?? { enabled: true },
      portal: undefined,
      tableMaxHeight: 300,
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
  /** The streaming build (rill's, or the page's engine's). */
  stream: LoadedStream;
  text: string;
  options: StreamOptions;
  /** The page's own components and plugins, and Streamdown's context (islands). */
  components: Record<string, unknown> | undefined;
  plugins: PluginConfig | undefined;
  context: Record<string, unknown>;
  className: string;
  attributes: Props;
  /** The text needs the exact path, or a button was clicked. */
  onHandOff(click: Replay | null): void;
};

/** Streamdown's root element, its children kept by rill-stream: each update renders only
 * the open tail and edits the DOM in place. */
function StreamRoot({
  stream,
  text,
  options,
  components,
  plugins,
  context,
  className,
  attributes,
  onHandOff,
}: StreamRootProps) {
  // (By value: a page's inline objects keep the islands as they were.)
  // biome-ignore lint/correctness/useExhaustiveDependencies: by value
  const env = useMemo(
    () => ({ components: components ?? EMPTY, plugins, context }),
    [components, plugins, ...Object.values(context)]
  );
  const root = useRef<HTMLDivElement>(null);
  const state = useRef<{
    renderer: StreamRenderer;
    session: StreamSession;
    key: string;
    animating: boolean;
    text: string;
  } | null>(null);
  const [islands, setIslands] = useState<Island[]>([]);
  const caret = useRef<Element | null>(null);
  // (`isAnimating` only changes the open tail and the controls' `disabled`.)
  // (A new build, as a new engine's, starts over too.)
  const key = JSON.stringify({ ...options, animating: undefined, build: buildId(stream) });
  useLayoutEffect(() => {
    const module = stream;
    let current = state.current;
    const shown = () => {
      // Streamdown's caret flag, when Rust gave it to an island: on the root's last
      // element, which the island's component rendered.
      const flag =
        options.overrides > 0 && loadedIslands()!.islandCaret(current!.renderer.islands());
      const last = flag ? root.current!.lastElementChild : null;
      if (caret.current !== last) {
        caret.current?.removeAttribute("data-sd-caret-hidden");
        last?.setAttribute("data-sd-caret-hidden", "");
        caret.current = last;
      }
      const next = current!.renderer.islands();
      if (options.overrides > 0 && loadedIslands()!.ownRenderer(next, plugins)) return onHandOff(null);
      if (next !== islands) setIslands(next);
    };
    if (current?.key === key && current.animating && !options.animating) {
      // The message finished: the same session, its tail rendered again.
      current.session.setOptions(options.controls, false);
      current.animating = false;
      root
        .current!.querySelectorAll("button[disabled]")
        .forEach((button) => button.removeAttribute("disabled"));
      current.text = text;
      if (!current.renderer.update(text)) onHandOff(null);
      else shown();
      return;
    }
    // Other options: the text renders again from the start.
    if (current?.key !== key || current.animating !== options.animating) {
      current?.renderer.dispose();
      root.current!.textContent = "";
      const session = module.newSession(options);
      const renderer = createStreamRenderer(root.current!, session, module, {
        islands: options.overrides > 0,
      });
      current = state.current = {
        renderer,
        session,
        key,
        animating: options.animating,
        text: "\0",
      };
    }
    // (A render for the islands alone leaves the text as it is.)
    if (current.text === text) return shown();
    current.text = text;
    if (!current.renderer.update(text)) onHandOff(null);
    else shown();
  });
  useEffect(
    () => () => {
      state.current?.renderer.dispose();
      state.current = null;
    },
    [],
  );
  const div = createElement("div", {
    ...attributes,
    ref: root,
    className,
    onClick: (event: MouseEvent<HTMLDivElement>) => {
      const element = root.current!;
      const button = (event.target as Element | null)?.closest?.("button");
      if (!button || !element.contains(button)) return;
      // (An island's own button is its component's.)
      for (
        let node: Node | null = button;
        node !== null && node !== element;
        node = node.parentNode
      ) {
        if (isIslandNode(node)) return;
      }
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
  if (islands.length === 0) return div;
  return createElement(Fragment, null, div, createElement(loadedIslands()!.IslandPortals, { islands, env }));
}
