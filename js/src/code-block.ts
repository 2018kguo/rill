// Fenced code blocks for the drop-in component: Streamdown's own shell (container,
// header, copy and download buttons, all exported by Streamdown) around a body whose
// highlighted lines are one HTML string instead of a React element per token. The body
// reproduces Streamdown 2.7.1's CodeBlockBody and HighlightedCodeBlockBody
// (lib/code-block/body.tsx, highlighted-body.tsx; Apache-2.0, Copyright 2023 Vercel,
// Inc.): raw lines first, then the code plugin's tokens once its callback (or cache)
// has them.
import {
  createElement,
  Fragment,
  memo,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ComponentType,
  type ReactNode,
  type RefObject,
} from "react";
import {
  CodeBlock,
  CodeBlockContainer,
  CodeBlockCopyButton,
  CodeBlockDownloadButton,
  CodeBlockHeader,
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
};

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
  const controls = codeControls(context.controls, code, language);
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
  return createElement(
    CodeBlockContainer,
    { dir: "ltr", isIncomplete, language },
    createElement(CodeBlockHeader, { language }),
    controls
      ? createElement(
          "div",
          {
            className: cn(
              "pointer-events-none absolute top-2 right-2 z-10 flex items-center"
            ),
          },
          createElement(
            "div",
            {
              className: cn(
                "pointer-events-auto flex shrink-0 items-center gap-2 rounded-md border border-sidebar bg-sidebar/80 px-1.5 py-1 supports-[backdrop-filter]:bg-sidebar/70 supports-[backdrop-filter]:backdrop-blur"
              ),
              "data-streamdown": "code-block-actions",
            },
            controls
          )
        )
      : null,
    createElement(CodeBody, {
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
  const raw = useMemo(() => rawResult(code), [code]);
  const [result, setResult] = useState<HighlightResult>(raw);
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
  const rows = useMemo(() => linesHtml(result, names, lines.current), [result, names]);
  // A block that arrives complete renders its <pre> as one string; one that streams
  // keeps a React element per line, so each update touches only the lines that changed.
  const [whole] = useState(!isIncomplete);
  const styles = useRef<{ pre?: StyleSlot; code?: StyleSlot }>({});
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
    // The <pre> and <code> styles as React would have updated them.
    const pre = (styles.current.pre = updateStyle(styles.current.pre, preStyle));
    const code = (styles.current.code = updateStyle(styles.current.code, codeStyle));
    let html = `<pre class="${escapeHtml(names.pre)}"`;
    if (pre.attribute !== null) html += ` style="${pre.attribute}"`;
    html += "><code";
    if (lineNumbers) html += ` class="${escapeHtml(names.code)}"`;
    if (code.attribute !== null) html += ` style="${code.attribute}"`;
    html += ">";
    const line = `<span class="${escapeHtml(lineClass)}">`;
    for (const row of rows) html += `${line}${row}</span>`;
    html += "</code></pre>";
    return createElement("div", { ...body, dangerouslySetInnerHTML: { __html: html } });
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
