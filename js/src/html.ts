// HTML as React writes it, for the markup the drop-in component sets with
// `dangerouslySetInnerHTML`: React's escaping, attribute names and styles. In the
// browser a style is what the CSSOM serializes once React has set it (React sets styles
// property by property, so the attribute reads back normalized); on the server it is
// React's own `name:value;name:value`.
import { clsx, type ClassValue } from "clsx";
import { find, html as htmlSchema, svg as svgSchema } from "property-information";
import styleToJs from "style-to-js";
import { twMerge } from "tailwind-merge";

import type { HastNode } from "./decode.ts";

const ESCAPE = /["&'<>]/g;
const NEEDS_ESCAPE = /["&'<>]/;
const ENTITIES: Record<string, string> = {
  '"': "&quot;",
  "&": "&amp;",
  "'": "&#x27;",
  "<": "&lt;",
  ">": "&gt;",
};

/** React's `escapeTextForBrowser`. */
export function escapeHtml(value: string): string {
  return NEEDS_ESCAPE.test(value)
    ? value.replace(ESCAPE, (character) => ENTITIES[character] ?? character)
    : value;
}

/** Streamdown's prefix-aware `cn` (lib/utils.ts `createCn`). */
export function createCn(prefix: string | undefined) {
  if (!prefix) {
    return (...inputs: ClassValue[]) => twMerge(clsx(inputs));
  }
  const own = `${prefix}:`;
  return (...inputs: ClassValue[]) =>
    twMerge(clsx(inputs))
      .split(/\s+/)
      .filter(Boolean)
      .map((name) => (name.startsWith(own) ? name : `${own}${name}`))
      .join(" ");
}

export type Cn = ReturnType<typeof createCn>;

let scratch: HTMLElement | undefined;
const cssomCache = new Map<string, string>();

/** The `style` attribute React leaves for `style` (an object of React style names). */
export function styleAttribute(
  style: Record<string, string>,
  ssr: boolean
): string {
  const names = Object.keys(style);
  if (ssr) {
    return names
      .map((name) => `${hyphenate(name)}:${escapeHtml(String(style[name]).trim())}`)
      .join(";");
  }
  let key = "";
  for (const name of names) {
    key += `${name}\u0000${style[name]}\u0000`;
  }
  let cached = cssomCache.get(key);
  if (cached === undefined) {
    scratch ??= document.createElement("div");
    scratch.removeAttribute("style");
    const declaration = scratch.style as unknown as Record<string, string> &
      CSSStyleDeclaration;
    for (const name of names) {
      const value = String(style[name]).trim();
      if (name.startsWith("--")) {
        declaration.setProperty(name, value);
      } else {
        declaration[name === "float" ? "cssFloat" : name] = value;
      }
    }
    cached = escapeHtml(scratch.getAttribute("style") ?? "");
    if (cssomCache.size > 4096) {
      cssomCache.clear();
    }
    cssomCache.set(key, cached);
  }
  return cached;
}

/** One element's `style` prop and the attribute React has left for it (escaped). */
export type StyleSlot = {
  style: Record<string, string> | undefined;
  attribute: string | null;
};

const transitions = new Map<string, string | null>();

function styleKey(style: Record<string, string> | undefined): string {
  if (style === undefined) return "\u0002";
  let key = "";
  for (const name of Object.keys(style)) key += `${name}\u0000${style[name]}\u0000`;
  return key;
}

function sameStyle(
  left: Record<string, string> | undefined,
  right: Record<string, string> | undefined
): boolean {
  if (left === right) return true;
  if (!(left && right)) return false;
  const names = Object.keys(left);
  if (names.length !== Object.keys(right).length) return false;
  for (const name of names) if (left[name] !== right[name]) return false;
  return true;
}

/**
 * The `style` attribute of an element React keeps from render to render (`slot`,
 * `undefined` for a new one) once it renders with `style`: React sets only what changed
 * and clears what is gone, so what the CSSOM rejects (Shiki's `bg` holds `;`) leaves
 * the previous value, and cleared styles leave `style=""`.
 */
export function updateStyle(
  slot: StyleSlot | undefined,
  style: Record<string, string> | undefined
): StyleSlot {
  if (slot === undefined) {
    const fresh = style && Object.keys(style).length > 0;
    return { style, attribute: fresh ? styleAttribute(style, false) : null };
  }
  if (sameStyle(slot.style, style)) return slot;
  const key = `${slot.attribute ?? "\u0002"}\u0001${styleKey(slot.style)}\u0001${styleKey(style)}`;
  let attribute = transitions.get(key);
  if (attribute === undefined) {
    scratch ??= document.createElement("div");
    if (slot.attribute === null) {
      scratch.removeAttribute("style");
    } else {
      scratch.setAttribute("style", unescapeHtml(slot.attribute));
    }
    const declaration = scratch.style as unknown as Record<string, string> &
      CSSStyleDeclaration;
    const set = (name: string, value: string) => {
      if (name.startsWith("--")) {
        declaration.setProperty(name, value);
      } else {
        declaration[name === "float" ? "cssFloat" : name] = value;
      }
    };
    const previous = slot.style ?? {};
    for (const name of Object.keys(previous)) {
      if (!(style && Object.hasOwn(style, name))) set(name, "");
    }
    for (const name of Object.keys(style ?? {})) {
      const value = (style as Record<string, string>)[name] as string;
      if (value !== previous[name]) set(name, String(value).trim());
    }
    const read = scratch.getAttribute("style");
    attribute = read === null ? null : escapeHtml(read);
    if (transitions.size > 4096) transitions.clear();
    transitions.set(key, attribute);
  }
  return { style, attribute };
}

function unescapeHtml(value: string): string {
  return value.includes("&")
    ? value
        .replaceAll("&quot;", '"')
        .replaceAll("&#x27;", "'")
        .replaceAll("&lt;", "<")
        .replaceAll("&gt;", ">")
        .replaceAll("&amp;", "&")
    : value;
}

/** React's `hyphenateStyleName`. */
function hyphenate(name: string): string {
  if (name.startsWith("--")) {
    return name;
  }
  return name
    .replace(/([A-Z])/g, "-$1")
    .toLowerCase()
    .replace(/^ms-/, "-ms-");
}

const VOID = new Set([
  "area",
  "base",
  "br",
  "col",
  "embed",
  "hr",
  "img",
  "input",
  "link",
  "meta",
  "source",
  "track",
  "wbr",
]);

/**
 * HAST elements without components of their own (KaTeX's output) as React renders
 * them: hast-util-to-jsx-runtime's props, then React's attributes. `ssr` adds
 * `<!-- -->` between adjacent text, as React's server renderer does.
 */
export function hastToHtml(nodes: HastNode[], ssr: boolean): string {
  let out = "";
  const write = (list: HastNode[], svg: boolean) => {
    let textBefore = false;
    for (const node of list) {
      if (node.type === "text") {
        const value = node.value ?? "";
        if (value === "") continue;
        if (ssr && textBefore) out += "<!-- -->";
        out += escapeHtml(value);
        textBefore = true;
        continue;
      }
      textBefore = false;
      if (node.type !== "element" || !node.tagName) continue;
      const tag = node.tagName;
      const inSvg = svg || tag.toLowerCase() === "svg";
      out += `<${tag}${attributes(node.properties ?? {}, inSvg, ssr)}`;
      if (VOID.has(tag) && !inSvg) {
        out += "/>";
        continue;
      }
      out += ">";
      write(node.children ?? [], inSvg);
      out += `</${tag}>`;
    }
  };
  write(nodes, false);
  return out;
}

function attributes(
  properties: Record<string, unknown>,
  svg: boolean,
  ssr: boolean
): string {
  let out = "";
  for (const [property, raw] of Object.entries(properties)) {
    if (raw === null || raw === undefined || Number.isNaN(raw)) continue;
    const info = find(svg ? svgSchema : htmlSchema, property);
    let value: unknown = raw;
    if (Array.isArray(value)) {
      value = value.join(info.commaSeparated ? ", " : " ");
    }
    if (info.property === "style") {
      let style: Record<string, string> = {};
      try {
        style = styleToJs(String(value), { reactCompat: true }) as Record<
          string,
          string
        >;
      } catch {
        // hast-util-to-jsx-runtime with `ignoreInvalidStyle`.
      }
      if (Object.keys(style).length > 0) {
        out += ` style="${styleAttribute(style, ssr)}"`;
      }
      continue;
    }
    const name = info.attribute;
    if (typeof value === "boolean") {
      if (name.startsWith("data-") || name.startsWith("aria-")) {
        out += ` ${name}="${value}"`;
      } else if (value && info.boolean) {
        out += ` ${name}=""`;
      }
      continue;
    }
    out += ` ${name}="${escapeHtml(String(value))}"`;
  }
  return out;
}
