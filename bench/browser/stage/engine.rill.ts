// rill's drop-in <Streamdown> (@2018kguo/rill/streamdown), from this repository's
// package sources and its built WebAssembly (build.ts copies js/wasm/rill_bg.wasm).
import { initRill } from "../../../js/src/index.ts";
import { setHighlightWasm } from "../../../js/src/streamdown.ts";

await initRill(await (await fetch("rill_bg.wasm")).arrayBuffer());
setHighlightWasm(new URL("rill_highlight.wasm", location.href));

export { Streamdown } from "../../../js/src/streamdown.ts";
export const engineName = "rill drop-in";

/** Props each answer adds (`settled`: it is not streaming; `text`: what it shows). */
export const answerProps = (_settled: boolean, _text: string): Record<string, unknown> => ({});
