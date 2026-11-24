// Islands for the streaming root: elements of tags the page's `components` override,
// rendered by React from the stream's tokens (see `Island` in stream-dom.ts). Loaded
// only for a message that passes `components`.
import { hastToReact, html as htmlSchema, find } from "property-information";
import { createElement, memo, type ComponentType, type ReactNode } from "react";
import { createPortal } from "react-dom";
import styleToJs from "style-to-js";

import type { Island, IslandNode } from "./stream-dom.ts";

type Components = Record<string, unknown>;

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

function render(node: IslandNode, components: Components): ReactNode {
  if (typeof node === "string") return node;
  const element = parse(node.html, node.inner !== undefined);
  const tag = element.localName;
  const given = props(element);
  if (node.inner !== undefined) {
    return createElement(tag, { ...given, dangerouslySetInnerHTML: { __html: element.innerHTML } });
  }
  const children = node.children.map((child) => render(child, components));
  if (!node.island) return createElement(tag, given, ...children);
  delete given[CARET];
  const component = (components[tag] ?? tag) as ComponentType<Record<string, unknown>> | string;
  const properties = Object.fromEntries(
    Array.from(element.attributes, ({ name, value }) => [name, value]),
  );
  delete properties[CARET];
  return createElement(
    component,
    { ...given, node: { type: "element", tagName: tag, properties, children: [] } },
    ...children,
  );
}

const IslandView = memo(
  function IslandView({ island, components }: { island: Island; components: Components }) {
    return render(island.tree, components);
  },
  (before, after) =>
    before.island.json === after.island.json && before.components === after.components,
);

/** Each island, rendered into its place. */
export function islandPortals(islands: Island[], components: Components): ReactNode[] {
  return islands.map((island) =>
    createPortal(
      createElement(IslandView, { island, components }),
      island.holder,
      String(island.key),
    ),
  );
}

/** Whether an island asks for Streamdown's caret flag on the root's last element. */
export function islandCaret(islands: Island[]): boolean {
  const last = islands.at(-1);
  return last !== undefined && last.tree.html.includes(` ${CARET}=`);
}
