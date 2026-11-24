// Islands for the streaming root: elements of tags the page's `components` override,
// and code blocks the page's `plugins.code` highlights, rendered by React from the
// stream's tokens (see `Island` in stream-dom.ts). Loaded only for a message that
// passes `components` or `plugins`.
// ponytail: one chunk, Streamdown's code block shell included; split it if a page with
// components alone needs a smaller download.
import { find, hastToReact, html as htmlSchema } from "property-information";
import { createElement, memo, type ComponentType, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { StreamdownContext, type PluginConfig } from "streamdown";
import styleToJs from "style-to-js";

import { CodeSegment, shellLabels } from "./code-block.ts";
import { highlighterFor } from "./highlight.ts";
import { createCn } from "./html.ts";
import type { Island, IslandNode } from "./stream-dom.ts";

type Components = Record<string, unknown>;

/** What islands render with: the page's components, and for code blocks its plugins. */
export type IslandEnv = {
  components: Components;
  plugins: PluginConfig | undefined;
  /** Streamdown's context for its components (code blocks read it). */
  context: Record<string, unknown>;
};

const cn = createCn(undefined);
const shell = shellLabels("");

const template = typeof document === "undefined" ? null : document.createElement("template");
const parsed = new Map<string, Element>();

/** The element an open tag (or, for a static subtree, its whole HTML) parses to. */
function parse(html: string, whole: boolean): Element {
  let element = parsed.get(html);
  if (element === undefined) {
    template!.innerHTML = whole ? html : `${html}</${html.slice(1, html.search(/[\s>]/))}>`;
    element = template!.content.firstElementChild!;
    parsed.set(html, element);
  }
  return element;
}

/** The React props for an element's attributes, as hast-util-to-jsx-runtime gives them. */
function props(element: Element): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const { name, value } of Array.from(element.attributes)) {
    if (name.startsWith("data-") || name.startsWith("aria-")) {
      out[name] = value;
      continue;
    }
    const info = find(htmlSchema, name);
    if (info.property === "style") {
      try {
        out.style = styleToJs(value, { reactCompat: true });
      } catch {
        // (hast-util-to-jsx-runtime's `ignoreInvalidStyle`.)
      }
      continue;
    }
    const property = (hastToReact as Record<string, string>)[info.property] ?? info.property;
    out[property] = info.boolean ? true : value;
  }
  return out;
}

/** Streamdown's caret flag on an island (set on the root's last element, not a prop). */
const CARET = "data-sd-caret-hidden";

function render(node: IslandNode, env: IslandEnv): ReactNode {
  if (typeof node === "string") return node;
  const { components } = env;
  const element = parse(node.html, node.inner !== undefined);
  const tag = element.localName;
  const given = props(element);
  if (node.inner !== undefined) {
    return createElement(tag, { ...given, dangerouslySetInnerHTML: { __html: element.innerHTML } });
  }
  if (node.island && tag === "code") {
    // A code block: rill's own (Streamdown's shell, the plugin's highlighting).
    const { "data-incomplete": incomplete, [CARET]: _caret, ...codeProps } = given;
    return createElement(CodeSegment, {
      code: node.children.join(""),
      codeProps,
      isIncomplete: incomplete === "true",
      ssr: false,
      plugins: env.plugins,
      components,
      cn,
      highlight: highlighterFor(env.plugins?.code),
      shell,
    });
  }
  const children = node.children.map((child) => render(child, env));
  if (!node.island) return createElement(tag, given, ...children);
  delete given[CARET];
  const component = (components[tag] ?? tag) as ComponentType<Record<string, unknown>> | string;
  const properties = Object.fromEntries(Array.from(element.attributes, ({ name, value }) => [name, value]));
  delete properties[CARET];
  return createElement(
    component,
    { ...given, node: { type: "element", tagName: tag, properties, children: [] } },
    ...children
  );
}

const IslandView = memo(
  function IslandView({ island, env }: { island: Island; env: IslandEnv }) {
    return render(island.tree, env);
  },
  (before, after) => before.island.json === after.island.json && before.env === after.env
);

/** Each island, rendered into its place, under Streamdown's context. */
export function IslandPortals({ islands, env }: { islands: Island[]; env: IslandEnv }): ReactNode {
  const portals = islands.map((island) =>
    createPortal(createElement(IslandView, { island, env }), island.holder, String(island.key))
  );
  return createElement(StreamdownContext.Provider, { value: env.context as never }, ...portals);
}

/** A code block the page renders with its own component (Mermaid, `renderers`): it
 * hands the message to Streamdown's components. */
export function ownRenderer(islands: Island[], plugins: PluginConfig | undefined): boolean {
  if (!plugins?.mermaid && !plugins?.renderers?.length) return false;
  return islands.some((island) => {
    const language = island.tree.html.match(/^<code class="language-([^"\s]+)/)?.[1];
    if (language === undefined) return false;
    if (language === "mermaid" && plugins.mermaid) return true;
    return Boolean(
      plugins.renderers?.some((r) => (Array.isArray(r.language) ? r.language.includes(language) : r.language === language))
    );
  });
}

/** Whether an island asks for Streamdown's caret flag on the root's last element. */
export function islandCaret(islands: Island[]): boolean {
  const last = islands.at(-1);
  return last !== undefined && last.tree.html.includes(` ${CARET}=`);
}
