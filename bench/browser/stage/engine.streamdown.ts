export { Streamdown } from "streamdown";
export const engineName = "Streamdown 2.7.1";

/** Props each answer adds (`settled`: it is not streaming; `text`: what it shows). */
export const answerProps = (_settled: boolean, _text: string): Record<string, unknown> => ({});

/** The code plugin the page passes as `plugins.code`. */
export { code } from "@streamdown/code";
