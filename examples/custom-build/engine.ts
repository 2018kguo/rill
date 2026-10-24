// The example build as an engine: its WebAssembly (src/lib.rs, built into pkg/ by
// build.sh) and the JavaScript plugins it reproduces, for each set of its arguments.
//
//   <Streamdown engine={customEngine} engineArgs={["/users", true]}>{text}</Streamdown>
import { math } from "@streamdown/math";
import { defaultRehypePlugins, defaultRemarkPlugins } from "streamdown";

import { defineEngine } from "../../js/src/engine.ts";
import { rehypeHashtags, rehypeMentions, remarkEmoji } from "./plugins.ts";

/** `base`: where `@name` links to (no mentions without it); `hashtags`: tag `#tag`. */
export const customEngine = defineEngine({
  glue: () => import("./pkg/custom.js"),
  wasm: () => new URL("./pkg/custom_bg.wasm", import.meta.url),
  plugins: (base: string | null, hashtags: boolean) => ({
    remarkPlugins: [...Object.values(defaultRemarkPlugins), math.remarkPlugin, remarkEmoji],
    rehypePlugins: [
      defaultRehypePlugins.raw!,
      ...(base === null ? [] : [[rehypeMentions, { base }] as const]),
      defaultRehypePlugins.sanitize!,
      defaultRehypePlugins.harden!,
      math.rehypePlugin,
      ...(hashtags ? [rehypeHashtags] : []),
    ],
  }),
  // The build's math (`Options::math`): rill leaves it to this entry of rehypePlugins.
  katex: math.rehypePlugin,
});
