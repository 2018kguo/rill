// Why a block or message renders with Streamdown's own `Block` (the slow path) instead
// of rill's markup. In development each reason is logged once per page; a listener
// (`setFallbackListener`) hears every fallback, in production too, for telemetry.

/**
 * Why rill left a block to Streamdown's own `Block`:
 *
 * - Props rill does not reproduce: `remarkPlugins`, `rehypePlugins` (lists that are not
 *   Streamdown's own, the engine's, or plugins rill knows), `allowedTags`,
 *   `literalTagContent`, `disableAutolinkProtocols`, `allowElement`, `allowedElements`,
 *   `disallowedElements`, `skipHtml`, `unwrapDisallowed`, `urlTransform`,
 *   `remarkRehypeOptions`, `fallbackComponent`, `static-dir` (`mode="static"` with `dir`),
 *   `engine-lists` (`<Streamdown engine>` with lists of its own), `single-dollar-math`
 *   (`plugins.math` without `singleDollarTextMath: false`), `math-twice` (an engine with
 *   math of its own and `plugins.math`), `animated-block-component` (`animated` with a
 *   `BlockComponent` of the page's own), `animated-options` (an `animation` or `easing`
 *   with characters only a CSS parser reads).
 * - The block's text: `raw-html` (raw HTML that needs the HTML5 parser), `too-deep`
 *   (nesting deeper than 256 levels), `too-long` (longer than 128K UTF-16 units),
 *   `surrogates` (lone surrogates or NUL), `plugin-declined` (a build's Rust plugin left
 *   it to its JavaScript form), `math-without-katex`, `animated-math` (math while
 *   `animated` streams), `unsupported-shape` (an element or property the markup does not
 *   reproduce), `incomplete-fence-components` and `incomplete-diagram` (a code fence
 *   still streaming next to the page's own components or diagram renderers).
 * - The WebAssembly: `loading` (not loaded yet; not logged), `server` (server rendering
 *   without `initRill` or `engine.init`), `load-failed`, `trapped` (it crashed; rill
 *   stops calling it), `error` (a render threw).
 */
export type FallbackReason =
  | "remarkPlugins"
  | "rehypePlugins"
  | "allowedTags"
  | "literalTagContent"
  | "disableAutolinkProtocols"
  | "allowElement"
  | "allowedElements"
  | "disallowedElements"
  | "skipHtml"
  | "unwrapDisallowed"
  | "urlTransform"
  | "remarkRehypeOptions"
  | "fallbackComponent"
  | "static-dir"
  | "engine-lists"
  | "single-dollar-math"
  | "math-twice"
  | "animated-block-component"
  | "animated-options"
  | "raw-html"
  | "too-deep"
  | "too-long"
  | "surrogates"
  | "plugin-declined"
  | "math-without-katex"
  | "animated-math"
  | "unsupported-shape"
  | "incomplete-fence-components"
  | "incomplete-diagram"
  | "loading"
  | "server"
  | "load-failed"
  | "trapped"
  | "error";

/** Hears every block rill leaves to Streamdown: the reason, and for plugin lists the
 * plugins rill does not know, by name. */
export type FallbackListener = (reason: FallbackReason, detail?: string) => void;

let listener: FallbackListener | null = null;

/** Calls `listen` for every block that renders with Streamdown's own `Block` (`null`
 * stops). In development rill also logs each reason once per page. */
export function setFallbackListener(listen: FallbackListener | null): void {
  listener = listen;
}

const DOCS = "https://github.com/2018kguo/rill#fallbacks";

/** The development log line for `reason`. */
function message(reason: FallbackReason, detail: string | undefined): string {
  const what = ((): string => {
    switch (reason) {
      case "remarkPlugins":
      case "rehypePlugins":
        return `${reason} given${detail ? ` (${detail})` : ""}`;
      case "allowedTags":
      case "literalTagContent":
      case "disableAutolinkProtocols":
      case "allowElement":
      case "allowedElements":
      case "disallowedElements":
      case "skipHtml":
      case "unwrapDisallowed":
      case "urlTransform":
      case "remarkRehypeOptions":
      case "fallbackComponent":
        return `${reason} given`;
      case "static-dir":
        return 'mode="static" with dir';
      case "engine-lists":
        return "<Streamdown engine> given remarkPlugins or rehypePlugins (the engine brings its own)";
      case "single-dollar-math":
        return "plugins.math without singleDollarTextMath: false";
      case "math-twice":
        return "an engine with math of its own and plugins.math";
      case "animated-block-component":
        return "animated with a BlockComponent of your own";
      case "animated-options":
        return "animated with an animation or easing only a CSS parser reads";
      case "raw-html":
        return "raw HTML that needs the HTML5 parser";
      case "too-deep":
        return "nesting deeper than 256 levels";
      case "too-long":
        return "a block longer than 128K characters";
      case "surrogates":
        return "lone surrogates or NUL in the text";
      case "plugin-declined":
        return "a Rust plugin of the engine declined the text";
      case "math-without-katex":
        return "math with no KaTeX plugin";
      case "animated-math":
        return "math while animated streams";
      case "unsupported-shape":
        return "an element or property rill does not reproduce";
      case "incomplete-fence-components":
        return "a streaming code fence beside components of your own";
      case "incomplete-diagram":
        return "a streaming code fence with diagram renderers";
      case "loading":
        return "the WebAssembly has not loaded yet";
      case "server":
        return "server rendering without initRill (or engine.init)";
      case "load-failed":
        return "the WebAssembly failed to load";
      case "trapped":
        return "the WebAssembly crashed, so rill stopped calling it";
      case "error":
        return `a render failed${detail ? ` (${detail})` : ""}`;
    }
  })();
  return `[rill] ${what}: these messages render with Streamdown (slow path). See ${DOCS}.`;
}

const logged = new Set<FallbackReason>();

/** Reports that a block renders with Streamdown's own `Block`, and why. */
export function reportFallback(reason: FallbackReason, detail?: string): void {
  if (listener) listener(reason, detail);
  if (
    typeof process !== "undefined" &&
    process.env.NODE_ENV !== "production" &&
    reason !== "loading" &&
    !logged.has(reason)
  ) {
    logged.add(reason);
    console.warn(message(reason, detail));
  }
}

/** For tests: logs every reason again. */
export function resetFallbackLog(): void {
  logged.clear();
}
