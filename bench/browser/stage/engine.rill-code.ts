// rill's drop-in with `code` from @2018kguo/rill/code (rill-highlight in place of
// @streamdown/code's Shiki): the page then ships no Shiki.
import { initRill } from "../../../js/src/index.ts";
import { setHighlightWasm } from "../../../js/src/streamdown.ts";

await initRill(await (await fetch("rill_bg.wasm")).arrayBuffer());
setHighlightWasm(new URL("rill_highlight.wasm", location.href));

export { Streamdown } from "../../../js/src/streamdown.ts";
export const engineName = "rill drop-in, rill/code";

/** Props each answer adds (`settled`: it is not streaming; `text`: what it shows). */
export const answerProps = (_settled: boolean, _text: string): Record<string, unknown> => ({});


export { code } from "../../../js/src/code.ts";
