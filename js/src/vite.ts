// `@2018kguo/rill/vite`: the Vite plugin, for Vite 7 and older (Vite 8 needs none, and
// it does nothing there but name the worker format).
//
// rill finds its WebAssembly and its highlighting worker with
// `new URL(..., import.meta.url)`. Vite 8 keeps those URLs pointing at the package when
// it pre-bundles dependencies for the dev server; Vite 7 does not (the URLs then point
// into node_modules/.vite and 404), so there rill and rill-highlight stay out of
// pre-bundling, and their dependencies, CommonJS ones among them, are pre-bundled on
// their own. The worker splits into chunks (one per grammar), which Rollup can only do
// for ES module workers.
//
//   // vite.config.ts
//   import { rill } from "@2018kguo/rill/vite";
//   export default defineConfig({ plugins: [react(), rill()] });

/** rill's runtime dependencies (package.json's `dependencies` and peers), pre-bundled
 * through it when rill itself is not. */
const DEPENDENCIES = [
  "clsx",
  "hast-util-to-jsx-runtime",
  "marked",
  "property-information",
  "remark-parse",
  "remark-rehype",
  "streamdown",
  "style-to-js",
  "tailwind-merge",
  "unified",
];

type ViteConfig = {
  optimizeDeps?: { exclude?: string[]; include?: string[] };
  worker?: { format?: "es" | "iife" };
};

/** The shape of a Vite plugin, without depending on Vite's types. */
export type RillVitePlugin = {
  name: string;
  config(this: { meta?: { viteVersion?: string } } | void): ViteConfig;
};

export function rill(): RillVitePlugin {
  return {
    name: "@2018kguo/rill",
    config() {
      const version = (this as { meta?: { viteVersion?: string } } | undefined)?.meta?.viteVersion;
      const major = version ? Number.parseInt(version, 10) : 0;
      const config: ViteConfig = { worker: { format: "es" } };
      if (!(major >= 8)) {
        config.optimizeDeps = {
          exclude: ["@2018kguo/rill", "@2018kguo/rill-highlight"],
          include: DEPENDENCIES.map((name) => `@2018kguo/rill > ${name}`),
        };
      }
      return config;
    },
  };
}

export default rill;
