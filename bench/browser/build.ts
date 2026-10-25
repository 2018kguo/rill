// Builds the stage for each engine into dist/<engine>: production React, minified, the
// page's Tailwind stylesheet, and (for rill) its WebAssembly and highlighting worker.
//
//   bun build.ts [streamdown] [rill] [name=path/to/engine.ts]
//
// An engine of your own (`name=file`) exports what engine.rill.ts does; it builds into
// dist/<name> with rill's WebAssembly and worker beside it, and loads its own assets
// (import them `with { type: "file" }`).
//
// rill is bundled from this repository's js/src and js/wasm. Every package both sides
// import (React, Streamdown, @streamdown/code, ...) resolves from this directory's
// node_modules, so the page has one copy of each and both engines get the same ones.
import type { BunPlugin } from "bun";
import { spawnSync } from "node:child_process";
import { cpSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";

const here = import.meta.dir;
const stage = join(here, "stage");
const js = join(here, "../../js");
const given = process.argv.slice(2).length ? process.argv.slice(2) : ["streamdown", "rill"];
/** Each engine's module: stage/engine.<name>.ts, or the file given as name=file. */
const files = new Map(
  given.map((arg) => {
    const [name, file] = arg.split("=") as [string, string | undefined];
    return [name, file ? resolve(file) : join(stage, `engine.${name}.ts`)];
  })
);
const engines = [...files.keys()];

const resolveHere = (engine: string): BunPlugin => ({
  name: "stage",
  setup(build) {
    // stage.tsx imports ./engine.ts; it is engine.<name>.ts.
    build.onResolve({ filter: /^\.\/engine\.ts$/ }, (args) =>
      args.importer === join(stage, "stage.tsx") ? { path: files.get(engine)! } : undefined
    );
    build.onResolve({ filter: /^[^./]/ }, (args) => {
      try {
        return { path: Bun.resolveSync(args.path, here) };
      } catch {
        return undefined;
      }
    });
  },
});

async function bundle(entry: string, outdir: string, engine: string) {
  const result = await Bun.build({
    entrypoints: [entry],
    outdir,
    target: "browser",
    splitting: true,
    // PROFILE=1: readable names for CPU profiles (whitespace and syntax still minified).
    minify: process.env.PROFILE ? { whitespace: true, syntax: true, identifiers: false } : true,
    define: { "process.env.NODE_ENV": '"production"' },
    plugins: [resolveHere(engine)],
  });
  if (!result.success) {
    for (const log of result.logs) console.error(log);
    process.exit(1);
  }
}

for (const engine of engines) {
  const dist = join(here, "dist", engine);
  rmSync(dist, { recursive: true, force: true });
  await bundle(join(stage, "stage.tsx"), dist, engine);
  const css = spawnSync("npx", ["--no-install", "@tailwindcss/cli", "-i", "input.css", "-o", join(dist, "stage.css"), "--minify"], { cwd: stage, encoding: "utf8" });
  if (css.status !== 0) {
    console.error(css.stderr);
    process.exit(1);
  }
  cpSync(join(stage, "stage.html"), join(dist, "stage.html"));
  cpSync(join(stage, "img"), join(dist, "img"), { recursive: true });
  if (engine !== "streamdown") {
    cpSync(join(js, "wasm/rill_bg.wasm"), join(dist, "rill_bg.wasm"));
    // The drop-in's highlighting worker and rill-highlight's WebAssembly, served next to
    // the bundle (js/src/highlight.ts loads both from there).
    await bundle(join(js, "src/highlight-worker.ts"), dist, engine);
    const highlight = dirname(Bun.resolveSync("@2018kguo/rill-highlight/package.json", js));
    cpSync(join(highlight, "dist/rill_highlight.wasm"), join(dist, "rill_highlight.wasm"));
  }
  console.log(`dist/${engine}`);
}
