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

import { escapeHtml, updateStyle, type Cn, type StyleSlot } from "./html.ts";

const LANGUAGE = /language-([^\s]+)/;
const START_LINE = /startLine=(\d+)/;
const NO_LINE_NUMBERS = /\bnoLineNumbers\b/;

/**
 * Highlights code as `plugins.code.highlight` does: a cached result, or `null` and the
 * result later through `callback`. The code plugin's own is the default; a faster one
 * must return the same tokens.
 */
export type Highlight = NonNullable<PluginConfig["code"]>["highlight"];

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
  highlight: Highlight | undefined;
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
  highlight: Highlight | undefined;
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
    const cached = highlight(
      {
        code,
        isIncomplete,
        language: language as never,
        themes: shikiTheme,
      },
      (highlighted) => {
        if (current) setResult(highlighted);
      }
    );
    if (cached) setResult(cached);
    return () => {
      current = false;
    };
  }, [code, isIncomplete, language, shikiTheme, highlight, raw]);
  const maxHeight = resolveMaxHeight(codeBlockMaxHeight);
  const scrollRef = usePinnedScroll(isAnimating, Boolean(maxHeight), result);
  // Each element's style as React would have updated it since the first render.
  const styles = useRef<BodyStyles>({ pre: undefined, code: undefined, rows: [] });
  const html = useMemo(
    () => bodyHtml(result, className, lineNumbers, startLine, cn, styles.current),
    [result, className, lineNumbers, startLine, cn]
  );
  return createElement("div", {
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
    dangerouslySetInnerHTML: { __html: html },
  });
});

/** The style slots of a body's elements, in React's element order. */
type BodyStyles = {
  pre: StyleSlot | undefined;
  code: StyleSlot | undefined;
  rows: StyleSlot[][];
};

type Classes = {
  pre: string;
  code: string;
  line: string;
  token: string;
  tokenWithBackground: string;
};
const classesByCn = new WeakMap<Cn, Map<string | undefined, Classes>>();

function classes(cn: Cn, className: string | undefined): Classes {
  let byClass = classesByCn.get(cn);
  if (!byClass) {
    byClass = new Map();
    classesByCn.set(cn, byClass);
  }
  let found = byClass.get(className);
  if (!found) {
    found = {
      pre: escapeHtml(
        cn(
          className,
          "bg-[var(--sdm-bg,inherit)]",
          "dark:bg-[var(--shiki-dark-bg,var(--sdm-bg,inherit))]"
        )
      ),
      code: escapeHtml(cn("[counter-increment:line_0] [counter-reset:line]")),
      line: "",
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

const LINE_CLASSES = "block";
const LINE_NUMBER_CLASSES =
  "block before:content-[counter(line)] before:inline-block before:[counter-increment:line] before:w-6 before:mr-4 before:text-[13px] before:text-right before:text-muted-foreground/50 before:font-mono before:select-none";
const lineClasses = new WeakMap<Cn, [string, string]>();

/** CodeBlockBody's `<pre>`, as HTML. */
function bodyHtml(
  result: HighlightResult,
  className: string | undefined,
  lineNumbers: boolean,
  startLine: number | undefined,
  cn: Cn,
  styles: BodyStyles
): string {
  const names = classes(cn, className);
  let lines = lineClasses.get(cn);
  if (!lines) {
    lines = [escapeHtml(cn(LINE_CLASSES)), escapeHtml(cn(LINE_NUMBER_CLASSES))];
    lineClasses.set(cn, lines);
  }
  const lineClass = lineNumbers ? lines[1] : lines[0];
  const preStyle: Record<string, string> = {};
  if (result.bg) preStyle["--sdm-bg"] = result.bg;
  if (result.fg) preStyle["--sdm-fg"] = result.fg;
  if (result.rootStyle) Object.assign(preStyle, parseRootStyle(result.rootStyle));
  let out = `<pre class="${names.pre}"`;
  styles.pre = updateStyle(styles.pre, preStyle);
  if (styles.pre.attribute !== null) out += ` style="${styles.pre.attribute}"`;
  out += "><code";
  if (lineNumbers) out += ` class="${names.code}"`;
  styles.code = updateStyle(
    styles.code,
    lineNumbers && startLine && startLine > 1
      ? { counterReset: `line ${startLine - 1}` }
      : undefined
  );
  if (styles.code.attribute !== null) out += ` style="${styles.code.attribute}"`;
  out += ">";
  const rows = result.tokens;
  styles.rows.length = Math.min(styles.rows.length, rows.length);
  for (let rowIndex = 0; rowIndex < rows.length; rowIndex += 1) {
    const row = rows[rowIndex] as HighlightResult["tokens"][number];
    out += `<span class="${lineClass}">`;
    const slots = (styles.rows[rowIndex] ??= []);
    if (row.length === 0 || (row.length === 1 && row[0]?.content === "")) {
      // Its token spans unmount.
      slots.length = 0;
      out += "\n";
    } else {
      slots.length = Math.min(slots.length, row.length);
      for (let tokenIndex = 0; tokenIndex < row.length; tokenIndex += 1) {
        const token = row[tokenIndex] as HighlightResult["tokens"][number][number];
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
        const slot = updateStyle(slots[tokenIndex], style);
        slots[tokenIndex] = slot;
        if (slot.attribute !== null) out += ` style="${slot.attribute}"`;
        if (token.htmlAttrs) {
          for (const [key, value] of Object.entries(token.htmlAttrs)) {
            out += ` ${key}="${escapeHtml(String(value))}"`;
          }
        }
        out += `>${escapeHtml(token.content)}</span>`;
      }
    }
    out += "</span>";
  }
  return `${out}</code></pre>`;
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
