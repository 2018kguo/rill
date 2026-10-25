// The scenarios behind the README's table: a name and the stage's query string.
export const SCENARIOS = {
  // Open a chat of 18 different long answers (chat.json: 15 Rust Book sections, three general).
  "hist-distinct": "scene=history&chat=distinct&overlay=0",
  // Open a code-heavy chat: the LRU-cache answer (four languages of code), 18 times.
  "hist-code": "scene=history&answers=lru-cache&overlay=0",
  // Stream the 118-element periodic table, 24 characters a frame.
  table118: "scene=stream&doc=elements-118&chunk=24&overlay=0",
  // Stream the Rust Book's chapter 4 (56 KB), 24 characters a frame.
  chapter: "scene=stream&doc=ownership-chapter&chunk=24&overlay=0",
  // Stream the chapter below a chat that already holds the 18 distinct answers.
  "after-chapter": "scene=after&chat=distinct&doc=ownership-chapter&chunk=24&overlay=0",
  // A realistic pace instead: 2,400 characters a second, below the same 18 answers.
  "after-paced": "scene=after&chat=distinct&doc=ownership-chapter&rate=2400&overlay=0",
};

/** `name` (one of SCENARIOS) or `name=query` to [name, query]. */
export function scenario(arg) {
  const split = arg.indexOf("=");
  if (split < 0) {
    if (!(arg in SCENARIOS)) throw new Error(`unknown scenario ${arg}; one of ${Object.keys(SCENARIOS).join(", ")}, or name=query`);
    return [arg, SCENARIOS[arg]];
  }
  return [arg.slice(0, split), arg.slice(split + 1)];
}

/** The engines `npm run build` builds, as [name, dist directory]. */
export const ENGINES = [
  ["streamdown", new URL("dist/streamdown", import.meta.url).pathname],
  ["rill", new URL("dist/rill", import.meta.url).pathname],
];
