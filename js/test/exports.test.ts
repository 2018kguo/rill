import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import * as streamdown from "streamdown";

import * as dropIn from "../src/streamdown.ts";

test("the drop-in exports everything Streamdown exports", () => {
  const missing = Object.keys(streamdown).filter((name) => !(name in dropIn));
  assert.deepEqual(missing, []);
});

test("the drop-in re-exports Streamdown's types by name", () => {
  // Every name in Streamdown's export list, types included (`export *` would carry them,
  // but Next.js rejects it in a "use client" module a server component imports).
  const types = readFileSync(new URL(import.meta.resolve("streamdown").replace(/\.js$/, ".d.ts")), "utf8");
  const list = types.match(/^export \{([^}]*)\};$/m)?.[1] ?? "";
  const names = list.split(",").map((name) => name.trim().replace(/^type /, "")).filter(Boolean);
  assert.ok(names.length > 50);
  const source = readFileSync(new URL("../src/streamdown.ts", import.meta.url), "utf8");
  assert.ok(!/^export \*/m.test(source), "no `export *` in the client module");
  const exported = (name: string) =>
    new RegExp(`\\b(type )?${name}\\b[,\\s]`).test(source.slice(source.indexOf("export {"))) ||
    new RegExp(`^export (const|type|function) ${name}\\b`, "m").test(source);
  assert.deepEqual(names.filter((name) => !exported(name)), []);
});
