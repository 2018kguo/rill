// Renders one scenario in each engine to the end (the stage's done rule, highlighting
// included) and compares the final #answer HTML byte for byte. Exits 1 if they differ.
//
//   node domcheck.mjs hist-distinct            (a name from scenarios.mjs, or name=query)
//   node domcheck.mjs hist-distinct dist/streamdown dist/rill
import { createHash } from "node:crypto";
import { chromium } from "playwright";

import { ENGINES, scenario } from "./scenarios.mjs";
import { serve } from "./trace-run.mjs";

const [name = "hist-distinct", ...dists] = process.argv.slice(2);
const [, query] = scenario(name);
const roots = dists.length ? dists : ENGINES.map(([, dist]) => dist);
const browser = await chromium.launch();
const out = [];
for (const root of roots) {
  const { origin, server } = serve(root);
  const page = await browser.newPage({ viewport: { width: 412, height: 915 } });
  await page.goto(`${origin}/stage.html?${query}`);
  await page.waitForSelector("body[data-ready]", { state: "attached" });
  await page.waitForLoadState("networkidle");
  await page.waitForTimeout(800);
  await page.evaluate(() => window.go());
  await page.waitForFunction(() => document.body.dataset.done, null, { timeout: 600_000 });
  const result = await page.evaluate(() => {
    const answer = document.querySelector("#answer");
    const body = '[data-streamdown="code-block-body"]';
    return {
      html: answer.innerHTML,
      // Highlighted tokens, and lines still raw (one token colored `inherit`).
      colored: answer.querySelectorAll(`${body} span[style*="--sdm-c"]:not([style*="inherit"])`).length,
      raw: answer.querySelectorAll(`${body} span[style*="inherit"]`).length,
    };
  });
  out.push(result);
  const hash = createHash("sha1").update(result.html).digest("hex").slice(0, 12);
  console.log(`${root}: ${hash}, ${result.html.length} chars, ${result.colored} colored tokens, ${result.raw} raw`);
  await page.close();
  server.close();
}
await browser.close();
if (out.length === 2 && out[0].html !== out[1].html) {
  const [a, b] = out.map((result) => result.html);
  let index = 0;
  while (a[index] === b[index]) index += 1;
  console.log(`first difference at ${index}\nA: ${a.slice(index - 200, index + 200)}\nB: ${b.slice(index - 200, index + 200)}`);
  process.exit(1);
}
if (out.length === 2) console.log("identical");
