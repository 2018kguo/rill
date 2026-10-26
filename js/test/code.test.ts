// `@2018kguo/rill/code`: the languages it answers from match rill-highlight's manifest,
// and its highlight (rill-highlight's plugin, loaded on first use) gives that plugin's
// tokens.
import assert from "node:assert/strict";
import { test } from "node:test";

const { default: manifest } = await import("@2018kguo/rill-highlight/dist/manifest.js");
const { createCodePlugin: highlightPlugin } = await import("@2018kguo/rill-highlight/streamdown");
const { code, createCodePlugin } = await import("../src/code.ts");

test("its languages are rill-highlight's (run scripts/languages.mjs after an upgrade)", () => {
  const ids = Object.keys(manifest.languages);
  assert.deepEqual([...code.getSupportedLanguages()].sort(), [...ids].sort());
  for (const [id, entry] of Object.entries(manifest.languages) as [string, { aliases: string[] }][]) {
    assert.ok(code.supportsLanguage(id as never), id);
    for (const alias of entry.aliases) assert.ok(code.supportsLanguage(alias as never), alias);
  }
  assert.ok(code.supportsLanguage(" TypeScript " as never));
  assert.ok(!code.supportsLanguage("not-a-language" as never));
});

test("it is a stock code plugin with @streamdown/code's default themes", () => {
  assert.equal(code.name, "shiki");
  assert.equal(code.type, "code-highlighter");
  assert.deepEqual(code.getThemes(), ["github-light", "github-dark"]);
  const themes = ["github-dark", "github-light"] as const;
  assert.deepEqual(createCodePlugin({ themes: [...themes] }).getThemes(), themes);
});

test("highlight loads rill-highlight on first use and gives its tokens", async () => {
  const request = { code: "const x = 1;\n", language: "ts", themes: ["github-light", "github-dark"] } as const;
  const plugin = createCodePlugin();
  const result = await new Promise((resolve) => {
    assert.equal(plugin.highlight(request as never, resolve), null);
  });
  const expected = await new Promise((resolve) => {
    const reference = highlightPlugin();
    const now = reference.highlight(request as never, resolve);
    if (now) resolve(now);
  });
  assert.deepEqual(result, expected);
  // Loaded, a second request comes back from rill-highlight's own cache, now.
  assert.deepEqual(plugin.highlight(request as never), expected);
});
