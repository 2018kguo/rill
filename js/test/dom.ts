// A DOM for tests that render the drop-in with React's client renderer (happy-dom), so
// memo and refs live across updates as they do in a browser. Import it before React.
// `render` returns the HTML and then the text nodes (React keeps adjacent strings apart).
import { Window } from "happy-dom";

const window = new Window({ url: "http://localhost/" });
const globals = globalThis as Record<string, unknown>;
for (const name of [
  "window",
  "document",
  "navigator",
  "Node",
  "Element",
  "HTMLElement",
  "Text",
  "DocumentFragment",
  "MutationObserver",
  "getComputedStyle",
  "requestAnimationFrame",
  "cancelAnimationFrame",
  "Event",
  "MouseEvent",
]) {
  globals[name] ??= (window as unknown as Record<string, unknown>)[name];
}
globals.IS_REACT_ACT_ENVIRONMENT = true;

const { createElement, act } = await import("react");
const { createRoot } = await import("react-dom/client");

/** The text nodes under `node`, each in brackets: where one text node ends and the
 * next begins, which the HTML does not show. */
function textNodes(node: Node): string {
  let out = "";
  for (let child = node.firstChild; child; child = child.nextSibling) {
    out += child.nodeType === 3 ? `[${child.nodeValue}]` : textNodes(child);
  }
  return out;
}

/** A container rendering `component` with React, updated with `render`. */
export function mount(component: unknown) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host as never);
  return {
    host,
    async render(children: string, props: Record<string, unknown> = {}) {
      await act(async () => {
        root.render(createElement(component as never, props, children));
      });
      return `${host.innerHTML}\n${textNodes(host)}`;
    },
    async unmount() {
      await act(async () => root.unmount());
      host.remove();
    },
  };
}
