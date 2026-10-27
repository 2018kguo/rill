// Fenced code blocks for the drop-in component: Streamdown's own shell (container,
// header, copy and download buttons, all exported by Streamdown) around a body whose
// highlighted lines are one HTML string instead of a React element per token. The body
// reproduces Streamdown 2.7.1's CodeBlockBody and HighlightedCodeBlockBody
// (lib/code-block/body.tsx, highlighted-body.tsx; Apache-2.0, Copyright 2023 Vercel,
// Inc.): raw lines first, then the code plugin's tokens once its callback (or cache)
// has them.
import { Lru } from "./prefix.ts";
import {
  createElement,
  Fragment,
  memo,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ComponentType,
  type MouseEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { flushSync } from "react-dom";
import {
  CodeBlock,
  CodeBlockContainer,
  CodeBlockCopyButton,
  CodeBlockDownloadButton,
  defaultTranslations,
  StreamdownContext,
  type ControlsConfig,
  type HighlightResult,
  type PluginConfig,
} from "streamdown";

import type { Highlighter } from "./highlight.ts";
import { escapeHtml, updateStyle, type Cn, type StyleSlot } from "./html.ts";

const LANGUAGE = /language-([^\s]+)/;
const START_LINE = /startLine=(\d+)/;
const NO_LINE_NUMBERS = /\bnoLineNumbers\b/;


export type CodeSegmentProps = {
  /** The `<code>`'s text, untrimmed. */
  code: string;
  /** The `<code>`'s props after hast-util-to-jsx-runtime (className, metastring). */
  codeProps: Record<string, unknown>;
  isIncomplete: boolean;
  /** Server rendering or hydrating it: Streamdown's own CodeBlock. */
  ssr: boolean;
  plugins: PluginConfig | undefined;
  components: Record<string, unknown>;
  cn: Cn;
  highlight: Highlighter | undefined;
  /** The controls' labels when they may start as static markup. */
  shell: ShellLabels | null;
};

/** What the code controls' markup depends on besides the prefix and controls. */
export type ShellLabels = { copyCode: string; downloadFile: string };

const labelsByKey = new Map<string, ShellLabels | null>();

/** The copy and download labels of `translations` (given as its JSON, as Streamdown keys
 * it), or `null` when they are not plain strings. */
export function shellLabels(translationsKey: string): ShellLabels | null {
  let labels = labelsByKey.get(translationsKey);
  if (labels === undefined) {
    const merged = {
      ...defaultTranslations,
      ...(translationsKey ? JSON.parse(translationsKey) : {}),
    } as Record<string, unknown>;
    const { copyCode, downloadFile } = merged;
    labels =
      typeof copyCode === "string" && typeof downloadFile === "string"
        ? { copyCode, downloadFile }
        : null;
    labelsByKey.set(translationsKey, labels);
  }
  return labels;
}

// Streamdown's control markup (lib/code-block/{copy,download}-button.tsx, lib/icons.tsx;
// Apache-2.0, Copyright 2023 Vercel, Inc.) as React writes it in the browser.
const ACTIONS_OUTER = "pointer-events-none absolute top-2 right-2 z-10 flex items-center";
const ACTIONS =
  "pointer-events-auto flex shrink-0 items-center gap-2 rounded-md border border-sidebar bg-sidebar/80 px-1.5 py-1 supports-[backdrop-filter]:bg-sidebar/70 supports-[backdrop-filter]:backdrop-blur";
const BUTTON =
  "cursor-pointer p-1 text-muted-foreground transition-all hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50";
const icon = (d: string) =>
  `<svg aria-hidden="true" color="currentColor" height="16" stroke-linejoin="round" viewBox="0 0 16 16" width="16" size="14"><path clip-rule="evenodd" d="${d}" fill="currentColor" fill-rule="evenodd"></path></svg>`;
const DOWNLOAD_ICON = icon(
  "M8.75 1V1.75V8.68934L10.7197 6.71967L11.25 6.18934L12.3107 7.25L11.7803 7.78033L8.70711 10.8536C8.31658 11.2441 7.68342 11.2441 7.29289 10.8536L4.21967 7.78033L3.68934 7.25L4.75 6.18934L5.28033 6.71967L7.25 8.68934V1.75V1H8.75ZM13.5 9.25V13.5H2.5V9.25V8.5H1V9.25V14C1 14.5523 1.44771 15 2 15H14C14.5523 15 15 14.5523 15 14V9.25V8.5H13.5V9.25Z"
);
const COPY_ICON = icon(
  "M2.75 0.5C1.7835 0.5 1 1.2835 1 2.25V9.75C1 10.7165 1.7835 11.5 2.75 11.5H3.75H4.5V10H3.75H2.75C2.61193 10 2.5 9.88807 2.5 9.75V2.25C2.5 2.11193 2.61193 2 2.75 2H8.25C8.38807 2 8.5 2.11193 8.5 2.25V3H10V2.25C10 1.2835 9.2165 0.5 8.25 0.5H2.75ZM7.75 4.5C6.7835 4.5 6 5.2835 6 6.25V13.75C6 14.7165 6.7835 15.5 7.75 15.5H13.25C14.2165 15.5 15 14.7165 15 13.75V6.25C15 5.2835 14.2165 4.5 13.25 4.5H7.75ZM7.5 6.25C7.5 6.11193 7.61193 6 7.75 6H13.25C13.3881 6 13.5 6.11193 13.5 6.25V13.75C13.5 13.8881 13.3881 14 13.25 14H7.75C7.61193 14 7.5 13.8881 7.5 13.75V6.25Z"
);

const actionsCache = new Map<string, string>();

/** The markup inside the controls' outer div, as CodeBlockDownloadButton and
 * CodeBlockCopyButton render it with the default icons, before either is pressed. */
function actionsHtml(cn: Cn, labels: ShellLabels, shown: Shown, disabled: boolean): string {
  const classes = cn(BUTTON);
  // The classes carry cn's prefix.
  const key = `${shown.download ? 1 : 0}${shown.copy ? 1 : 0}${disabled ? 1 : 0}${classes}\0${labels.copyCode}\0${labels.downloadFile}`;
  const known = actionsCache.get(key);
  if (known !== undefined) return known;
  const button = (kind: string, label: string, svg: string) =>
    `<button aria-label="${escapeHtml(label)}" class="${escapeHtml(classes)}" data-streamdown="${kind}"${disabled ? ' disabled=""' : ""} title="${escapeHtml(label)}" type="button">${svg}</button>`;
  let html = `<div class="${escapeHtml(cn(ACTIONS))}" data-streamdown="code-block-actions">`;
  if (shown.download) html += button("code-block-download-button", labels.downloadFile, DOWNLOAD_ICON);
  if (shown.copy) html += button("code-block-copy-button", labels.copyCode, COPY_ICON);
  html += "</div>";
  actionsCache.set(key, html);
  return html;
}

const CONTROL_BUTTON = "button[data-streamdown]";

/** A fenced code block as Streamdown's `code` component renders it. */
export function CodeSegment(props: CodeSegmentProps): ReactNode {
  const { code, codeProps, isIncomplete, ssr, plugins, components, cn } = props;
  const context = useContext(StreamdownContext);
  const className = codeProps.className as string | undefined;
  const language = className?.match(LANGUAGE)?.at(1) ?? "";
  const renderer =
    plugins?.renderers && language
      ? plugins.renderers.find((candidate) =>
          Array.isArray(candidate.language)
            ? candidate.language.includes(language)
            : candidate.language === language
        )
      : undefined;
  if (renderer || (language === "mermaid" && plugins?.mermaid)) {
    // Streamdown's own code component, as `pre` hands it the `<code>`.
    const node = {
      type: "element",
      tagName: "code",
      properties: { ...codeProps, className: className?.split(" ") },
      children: [{ type: "text", value: code }],
    };
    return createElement(
      components.code as ComponentType<Record<string, unknown>>,
      { ...codeProps, node, "data-block": "true" },
      code
    );
  }
  const { className: _className, ...forwarded } = codeProps;
  const metastring = codeProps.metastring as string | undefined;
  const startMatch = metastring?.match(START_LINE);
  const parsedStart = startMatch?.[1]
    ? Number.parseInt(startMatch[1], 10)
    : undefined;
  const startLine =
    parsedStart !== undefined && parsedStart >= 1 ? parsedStart : undefined;
  const lineNumbers =
    !(metastring ? NO_LINE_NUMBERS.test(metastring) : false) &&
    context.lineNumbers !== false;
  const shown = shownControls(context.controls);
  // The controls start as markup (no React elements for the buttons and their icons)
  // and turn into Streamdown's own buttons when one is first pressed, which then gets
  // the click (and the focus, if it had it).
  const [active, setActive] = useState(false);
  const shell = !ssr && !active && props.shell && shown ? props.shell : null;
  const controls = shell ? null : codeControls(context.controls, code, language);
  if (ssr) {
    return createElement(
      CodeBlock,
      {
        className,
        code,
        isIncomplete,
        language,
        lineNumbers,
        startLine,
        ...forwarded,
      } as never,
      controls
    );
  }
  const activate = (event: MouseEvent<HTMLDivElement>) => {
    const button = (event.target as Element | null)?.closest?.(CONTROL_BUTTON) as
      | HTMLButtonElement
      | null
      | undefined;
    const scope = event.currentTarget;
    if (!(button && scope.contains(button)) || button.disabled) return;
    const kind = button.getAttribute("data-streamdown");
    const focused = button.ownerDocument.activeElement === button;
    const container = scope.parentElement;
    event.stopPropagation();
    flushSync(() => setActive(true));
    const live = container?.querySelector<HTMLButtonElement>(
      `button[data-streamdown="${kind}"]`
    );
    if (focused) live?.focus();
    live?.click();
  };
  const outer = cn(ACTIONS_OUTER);
  return createElement(
    CodeBlockContainer,
    { dir: "ltr", isIncomplete, language },
    // CodeBlockHeader's markup.
    createElement("div", {
      key: "header",
      className: cn("flex h-8 items-center text-muted-foreground text-xs"),
      "data-language": language,
      "data-streamdown": "code-block-header",
      dangerouslySetInnerHTML: {
        __html: `<span class="${escapeHtml(cn("ml-1 font-mono lowercase"))}">${escapeHtml(language)}</span>`,
      },
    }),
    shell
      ? createElement("div", {
          key: "controls:html",
          className: outer,
          onClick: activate,
          dangerouslySetInnerHTML: {
            __html: actionsHtml(cn, shell, shown!, Boolean(context.isAnimating)),
          },
        })
      : controls
        ? createElement(
            "div",
            { key: "controls", className: outer },
            createElement(
              "div",
              {
                className: cn(ACTIONS),
                "data-streamdown": "code-block-actions",
              },
              controls
            )
          )
        : null,
    createElement(CodeBody, {
      key: "body",
      className,
      code: trimTrailingNewlines(code),
      isIncomplete,
      language,
      lineNumbers,
      startLine,
      rest: forwarded,
      cn,
      highlight: props.highlight,
    })
  );
}

type Shown = { download: boolean; copy: boolean };

/** lib/components.tsx: which of the download and copy buttons the code controls allow,
 * or `null` for neither. */
function shownControls(config: ControlsConfig): Shown | null {
  if (typeof config === "boolean") return config ? { download: true, copy: true } : null;
  const code = config.code;
  if (code === false) return null;
  if (code === true || code === undefined) return { download: true, copy: true };
  return { download: code.download !== false, copy: code.copy !== false };
}

/** lib/components.tsx: the download and copy buttons the code controls allow. */
function codeControls(
  config: ControlsConfig,
  code: string,
  language: string
): ReactNode {
  const show = (control: "copy" | "download") => {
    if (typeof config === "boolean") return config;
    const code = config.code;
    if (code === false) return false;
    if (code === true || code === undefined) return true;
    return code[control] !== false;
  };
  const shown = typeof config === "boolean" ? config : config.code !== false;
  if (!shown) {
    return null;
  }
  return createElement(
    Fragment,
    null,
    show("download")
      ? createElement(CodeBlockDownloadButton, { code, language })
      : null,
    show("copy")
      ? createElement(CodeBlockCopyButton, { code, ...copyCallbacks(config) } as never)
      : null
  );
}

/** lib/controls.ts `getCopyCallbacks(config, "code")`. */
function copyCallbacks(config: ControlsConfig) {
  if (typeof config === "boolean") return {};
  const code = config.code;
  if (typeof code !== "object") return {};
  const copy = code.copy;
  if (typeof copy !== "object" || copy === null) return {};
  return { onCopy: copy.onCopy, onError: copy.onError };
}

function trimTrailingNewlines(code: string): string {
  let end = code.length;
  while (end > 0 && code[end - 1] === "\n") end -= 1;
  return code.slice(0, end);
}

type CodeBodyProps = {
  className: string | undefined;
  code: string;
  isIncomplete: boolean;
  language: string;
  lineNumbers: boolean;
  startLine: number | undefined;
  rest: Record<string, unknown>;
  cn: Cn;
  highlight: Highlighter | undefined;
};

/** The callback of a request made while rendering (its result is cached for the body). */
const prefetched = () => {};

/** CodeBlock's raw result: each line one uncolored token. */
function rawResult(code: string): HighlightResult {
  return {
    bg: "transparent",
    fg: "inherit",
    tokens: code.split("\n").map((line) => [
      {
        content: line,
        color: "inherit",
        bgColor: "transparent",
        htmlStyle: {},
        offset: 0,
      },
    ]),
  } as HighlightResult;
}

const CodeBody = memo(function CodeBody({
  className,
  code,
  isIncomplete,
  language,
  lineNumbers,
  startLine,
  rest,
  cn,
  highlight,
}: CodeBodyProps) {
  const { isAnimating, shikiTheme, codeBlockMaxHeight } =
    useContext(StreamdownContext);
  const raw = useMemo(() => rawResultFor(code), [code]);
  const [result, setResult] = useState<HighlightResult>(raw);
  // A finished block asks for its tokens as it renders, so the worker tokenizes while the
  // page commits and lays out; the effect below then finds them, or joins the request.
  useMemo(() => {
    if (highlight && !isIncomplete) {
      highlight.highlight(
        { code, isIncomplete, language: language as never, themes: shikiTheme },
        prefetched
      );
    }
  }, [code, isIncomplete, language, shikiTheme, highlight]);
  useEffect(() => {
    if (!highlight) {
      setResult(raw);
      return;
    }
    // Ignore results for code this block has already moved past.
    let current = true;
    const callback = (highlighted: HighlightResult) => {
      if (current) setResult(highlighted);
    };
    const cached = highlight.highlight(
      {
        code,
        isIncomplete,
        language: language as never,
        themes: shikiTheme,
      },
      callback
    );
    if (cached) setResult(cached);
    return () => {
      current = false;
      highlight.cancel(callback);
    };
  }, [code, isIncomplete, language, shikiTheme, highlight, raw]);
  const maxHeight = resolveMaxHeight(codeBlockMaxHeight);
  const scrollRef = usePinnedScroll(isAnimating, Boolean(maxHeight), result);
  // The token spans' styles as React would have updated them since the first render,
  // and each line's HTML while its tokens stay the same.
  const lines = useRef<Line[]>([]);
  const names = classes(cn, className);
  const preStyle: Record<string, string> = {};
  if (result.bg) preStyle["--sdm-bg"] = result.bg;
  if (result.fg) preStyle["--sdm-fg"] = result.fg;
  if (result.rootStyle) Object.assign(preStyle, parseRootStyle(result.rootStyle));
  const lineClass = lineNumbers ? names.lineNumbers : names.line;
  const codeStyle =
    lineNumbers && startLine && startLine > 1
      ? { counterReset: `line ${startLine - 1}` }
      : undefined;
  // A block that arrives complete renders its <pre> as one string; one that streams
  // keeps a React element per line, so each update touches only the lines that changed.
  const [whole] = useState(!isIncomplete);
  const rows = useMemo(
    () => (whole ? [] : linesHtml(result, names, lines.current)),
    [result, names, whole]
  );
  const wholeState = useRef<WholeState>({
    memo: null,
    body: { lines: [], pre: undefined, code: undefined },
    shared: false,
  });
  const body = {
    className: cn(
      className,
      maxHeight ? "overflow-y-auto" : null,
      "overflow-x-auto rounded-md border border-border bg-background p-4 text-sm"
    ),
    "data-language": language,
    "data-streamdown": "code-block-body",
    ref: scrollRef,
    style: maxHeight ? { maxHeight } : undefined,
    ...rest,
  };
  if (whole) {
    // Everything the markup reads besides the result and the state it carries.
    const key = `${names.pre}\0${lineNumbers ? names.code : ""}\0${lineClass}\0${names.token}\0${names.tokenWithBackground}\0${startLine}`;
    const html = wholeHtml(wholeState.current, result, key, (state) => {
      // The <pre> and <code> styles as React would have updated them.
      const pre = (state.pre = updateStyle(state.pre, preStyle));
      const code = (state.code = updateStyle(state.code, codeStyle));
      let html = `<pre class="${escapeHtml(names.pre)}"`;
      if (pre.attribute !== null) html += ` style="${pre.attribute}"`;
      html += "><code";
      if (lineNumbers) html += ` class="${escapeHtml(names.code)}"`;
      if (code.attribute !== null) html += ` style="${code.attribute}"`;
      html += ">";
      const line = `<span class="${escapeHtml(lineClass)}">`;
      for (const row of linesHtml(result, names, state.lines)) html += `${line}${row}</span>`;
      return `${html}</code></pre>`;
    });
    return createElement(WholeBody, { body, html });
  }
  return createElement(
    "div",
    body,
    createElement(
      "pre",
      { className: names.pre, style: preStyle },
      createElement(
        "code",
        { className: lineNumbers ? names.code : undefined, style: codeStyle },
        rows.map((html, index) =>
          createElement("span", {
            className: lineClass,
            key: index,
            dangerouslySetInnerHTML: { __html: html },
          })
        )
      )
    )
  );
});

/** What a whole body's markup depends on besides its result: its rows' token styles
 * and its <pre> and <code> styles, as React would have updated them. */
type BodyState = { lines: Line[]; pre: StyleSlot | undefined; code: StyleSlot | undefined };

/** A body's markup after a sequence of results, and the state it leaves. */
type Memo = {
  html: string;
  body: BodyState;
  next: WeakMap<HighlightResult, Map<string, Memo>>;
  /** Results in the sequence: the raw lines (1), then the highlighted ones (2). */
  depth: number;
};

/** A body past the sequences worth sharing (one whose code keeps changing). */
const UNSHARED: Memo = { html: "", body: { lines: [], pre: undefined, code: undefined }, next: new WeakMap(), depth: Infinity };

type WholeState = { memo: Memo | null; body: BodyState; shared: boolean };

/** Bodies' markup by the results they rendered, in order (the raw lines, then the
 * highlighted ones): every message holding the same code renders it once. */
const firstMemo = new WeakMap<HighlightResult, Map<string, Memo>>();

function wholeHtml(
  state: WholeState,
  result: HighlightResult,
  key: string,
  render: (body: BodyState) => string
): string {
  // Only the raw-then-highlighted sequence repeats across messages. A body whose code
  // keeps changing (streaming without isAnimating) would chain a memo per update, each
  // holding that update's whole markup: it renders without one.
  if (state.memo && state.memo.depth >= 2) {
    const body = state.shared ? cloneBody(state.body) : state.body;
    const html = render(body);
    state.memo = UNSHARED;
    state.body = body;
    state.shared = false;
    return html;
  }
  const table = state.memo ? state.memo.next : firstMemo;
  let byKey = table.get(result);
  let memo = byKey?.get(key);
  if (!memo) {
    // A state another body shares stays as it is.
    const body = state.shared ? cloneBody(state.body) : state.body;
    memo = { html: render(body), body, next: new WeakMap(), depth: (state.memo?.depth ?? 0) + 1 };
    if (!byKey) table.set(result, (byKey = new Map()));
    byKey.set(key, memo);
  }
  state.memo = memo;
  state.body = memo.body;
  state.shared = true;
  return memo.html;
}

function cloneBody(body: BodyState): BodyState {
  return {
    lines: body.lines.map((line) => ({ ...line, slots: line.slots.slice() })),
    pre: body.pre,
    code: body.code,
  };
}

/** Raw results by code, so bodies with the same code share them (and their markup). */
// Bounded by bytes too: a code block streaming without isAnimating offers every prefix.
const rawResults = new Lru<string, HighlightResult>(256, 4 << 20, (code) => 12 * code.length);

function rawResultFor(code: string): HighlightResult {
  let result = rawResults.get(code);
  if (!result) {
    result = rawResult(code);
    rawResults.set(code, result);
  }
  return result;
}

/**
 * A body that arrives complete, its markup set outside React as dangerouslySetInnerHTML
 * would, in the same commit; markup set before (the same code in another message, a
 * message mounted again) is cloned from the first parse instead of parsed again.
 */
function WholeBody({ body, html }: { body: Record<string, unknown>; html: string }) {
  const element = useRef<HTMLDivElement | null>(null);
  const shown = useRef<string | null>(null);
  const scrollRef = body.ref as RefObject<HTMLDivElement | null>;
  useLayoutEffect(() => {
    const host = element.current;
    if (!host || shown.current === html) return;
    shown.current = html;
    setMarkup(host, html);
  }, [html]);
  return createElement("div", {
    ...body,
    ref: (node: HTMLDivElement | null) => {
      element.current = node;
      scrollRef.current = node;
    },
  });
}

/** Where markup set as a code body was parsed: the body that holds it, while it does. */
type Parsed = { host: WeakRef<HTMLElement>; bytes: number };

/** Markup set as code bodies, least recently used first, up to `PARSED_BYTES`. */
const parsed = new Map<string, Parsed>();
let parsedBytes = 0;
const PARSED_BYTES = 8 << 20;
/** The markup each body shows. */
const shownIn = new WeakMap<HTMLElement, string>();

function setMarkup(host: HTMLElement, html: string) {
  const known = parsed.get(html);
  const source = known?.host.deref();
  if (source && source !== host && source.isConnected && shownIn.get(source) === html) {
    // A copy of the DOM parsed from it (the same nodes, without parsing).
    parsed.delete(html);
    parsed.set(html, known!);
    const copy = document.createDocumentFragment();
    for (let node = source.firstChild; node; node = node.nextSibling) {
      copy.append(node.cloneNode(true));
    }
    host.replaceChildren(copy);
  } else {
    host.innerHTML = html;
    if (known) parsedBytes -= known.bytes;
    parsed.delete(html);
    parsed.set(html, { host: new WeakRef(host), bytes: html.length });
    parsedBytes += html.length;
  }
  shownIn.set(host, html);
  while (parsedBytes > PARSED_BYTES) {
    const [oldest, entry] = parsed.entries().next().value as [string, Parsed];
    parsed.delete(oldest);
    parsedBytes -= entry.bytes;
  }
}

type Row = HighlightResult["tokens"][number];

/** A line's tokens, their style slots, and its HTML. */
type Line = { row: Row; slots: StyleSlot[]; html: string };

type Classes = {
  pre: string;
  code: string;
  line: string;
  lineNumbers: string;
  token: string;
  tokenWithBackground: string;
};
const classesByCn = new WeakMap<Cn, Map<string | undefined, Classes>>();

const LINE_CLASSES = "block";
const LINE_NUMBER_CLASSES =
  "block before:content-[counter(line)] before:inline-block before:[counter-increment:line] before:w-6 before:mr-4 before:text-[13px] before:text-right before:text-muted-foreground/50 before:font-mono before:select-none";

/** CodeBlockBody's classes for `cn` (React elements' unescaped, tokens' escaped). */
function classes(cn: Cn, className: string | undefined): Classes {
  let byClass = classesByCn.get(cn);
  if (!byClass) {
    byClass = new Map();
    classesByCn.set(cn, byClass);
  }
  let found = byClass.get(className);
  if (!found) {
    found = {
      pre: cn(
        className,
        "bg-[var(--sdm-bg,inherit)]",
        "dark:bg-[var(--shiki-dark-bg,var(--sdm-bg,inherit))]"
      ),
      code: cn("[counter-increment:line_0] [counter-reset:line]"),
      line: cn(LINE_CLASSES),
      lineNumbers: cn(LINE_NUMBER_CLASSES),
      token: escapeHtml(
        cn(
          "text-[var(--sdm-c,inherit)]",
          "dark:text-[var(--shiki-dark,var(--sdm-c,inherit))]"
        )
      ),
      tokenWithBackground: escapeHtml(
        cn(
          "text-[var(--sdm-c,inherit)]",
          "dark:text-[var(--shiki-dark,var(--sdm-c,inherit))]",
          "bg-[var(--sdm-tbg)]",
          "dark:bg-[var(--shiki-dark-bg,var(--sdm-tbg))]"
        )
      ),
    };
    byClass.set(className, found);
  }
  return found;
}

/** Each line's token spans as HTML (CodeBlockBody's rows), reusing `lines` where a
 * line's tokens are the ones it rendered last time (Shiki keeps finished rows). */
function linesHtml(result: HighlightResult, names: Classes, lines: Line[]): string[] {
  const rows = result.tokens;
  lines.length = Math.min(lines.length, rows.length);
  const out = new Array<string>(rows.length);
  for (let index = 0; index < rows.length; index += 1) {
    const row = rows[index] as Row;
    let line = lines[index];
    if (line?.row === row) {
      out[index] = line.html;
      continue;
    }
    // A line React keeps keeps its token spans (and their styles).
    const slots = line?.slots ?? [];
    line = { row, slots, html: lineHtml(row, names, slots) };
    lines[index] = line;
    out[index] = line.html;
  }
  return out;
}

function lineHtml(row: Row, names: Classes, slots: StyleSlot[]): string {
  if (row.length === 0 || (row.length === 1 && row[0]?.content === "")) {
    // Its token spans unmount.
    slots.length = 0;
    return "\n";
  }
  slots.length = Math.min(slots.length, row.length);
  let out = "";
  for (let index = 0; index < row.length; index += 1) {
    const token = row[index] as Row[number];
    const style: Record<string, string> = {};
    let background = Boolean(token.bgColor);
    if (token.color) style["--sdm-c"] = token.color;
    if (token.bgColor) style["--sdm-tbg"] = token.bgColor;
    if (token.htmlStyle) {
      for (const [key, value] of Object.entries(token.htmlStyle)) {
        if (key === "color") {
          style["--sdm-c"] = value;
        } else if (key === "background-color") {
          style["--sdm-tbg"] = value;
          background = true;
        } else {
          style[key] = value;
        }
      }
    }
    out += `<span class="${background ? names.tokenWithBackground : names.token}"`;
    const slot = updateStyle(slots[index], style);
    slots[index] = slot;
    if (slot.attribute !== null) out += ` style="${slot.attribute}"`;
    if (token.htmlAttrs) {
      for (const [key, value] of Object.entries(token.htmlAttrs)) {
        out += ` ${key}="${escapeHtml(String(value))}"`;
      }
    }
    out += `>${escapeHtml(token.content)}</span>`;
  }
  return out;
}

function parseRootStyle(rootStyle: string): Record<string, string> {
  const style: Record<string, string> = {};
  for (const declaration of rootStyle.split(";")) {
    const index = declaration.indexOf(":");
    if (index > 0) {
      const property = declaration.slice(0, index).trim();
      const value = declaration.slice(index + 1).trim();
      if (property && value) style[property] = value;
    }
  }
  return style;
}

/** lib/use-pinned-scroll.ts `resolveMaxHeight`. */
export function resolveMaxHeight(
  maxHeight: number | string | undefined
): string | undefined {
  if (
    maxHeight === undefined ||
    maxHeight === 0 ||
    maxHeight === Number.POSITIVE_INFINITY
  ) {
    return;
  }
  if (typeof maxHeight === "number") {
    if (!(Number.isFinite(maxHeight) && maxHeight > 0)) return;
    return `${maxHeight}px`;
  }
  if (maxHeight === "0" || maxHeight === "none" || maxHeight === "Infinity") {
    return;
  }
  return maxHeight;
}

const BOTTOM_THRESHOLD_PX = 8;

/** lib/use-pinned-scroll.ts `usePinnedScroll`: keeps a streaming body scrolled to the
 * bottom until the reader scrolls up. */
function usePinnedScroll(
  isActive: boolean,
  enabled: boolean,
  contentKey: unknown
): RefObject<HTMLDivElement | null> {
  const scrollRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);
  const wasActiveRef = useRef(false);
  const frameRef = useRef<number | null>(null);
  useEffect(() => {
    const element = scrollRef.current;
    if (!(element && enabled)) return;
    const handleScroll = () => {
      pinnedRef.current =
        element.scrollHeight - element.scrollTop - element.clientHeight <
        BOTTOM_THRESHOLD_PX;
    };
    element.addEventListener("scroll", handleScroll, { passive: true });
    return () => element.removeEventListener("scroll", handleScroll);
  }, [enabled]);
  useEffect(() => {
    if (isActive && !wasActiveRef.current) pinnedRef.current = true;
    if (!isActive) pinnedRef.current = true;
    wasActiveRef.current = isActive;
  }, [isActive]);
  useEffect(
    () => () => {
      if (frameRef.current !== null) cancelAnimationFrame(frameRef.current);
    },
    []
  );
  // biome-ignore lint/correctness/useExhaustiveDependencies: contentKey detects growth
  useEffect(() => {
    const element = scrollRef.current;
    if (
      !(element && enabled && isActive && pinnedRef.current) ||
      frameRef.current !== null
    ) {
      return;
    }
    frameRef.current = requestAnimationFrame(() => {
      frameRef.current = null;
      if (pinnedRef.current) {
        element.scrollTo({ top: element.scrollHeight, behavior: "instant" });
      }
    });
  }, [isActive, enabled, contentKey]);
  return scrollRef;
}
