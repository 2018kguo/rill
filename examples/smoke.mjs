// Smoke test for an example app: `node examples/smoke.mjs http://localhost:3000`.
// The page must arrive server-rendered (where the example renders on the server), load
// rill's WebAssembly, highlight code through rill-highlight's worker, stream an answer,
// and log no errors or warnings (hydration mismatches included) on the way.
import { chromium } from "playwright";

const url = process.argv[2];
const ssr = !process.argv.includes("--no-ssr");
const warmup = process.argv.includes("--warmup");
if (!url) throw new Error("usage: node smoke.mjs <url> [--no-ssr] [--warmup]");

class Failure extends Error {}
const fail = (message) => {
  throw new Failure(message);
};

// Server rendering: the settled answer is in the HTML before any script runs.
if (ssr) {
  const html = await (await fetch(url)).text();
  if (!html.includes('data-streamdown="heading-1"') || !html.includes("Settled answer")) {
    console.error("smoke: the settled answer is not in the server-rendered HTML");
    process.exit(1);
  }
}

const browser = await chromium.launch();
if (warmup) {
  // A dev server that optimizes dependencies on first load reloads the page once.
  const first = await browser.newPage();
  await first.goto(url, { waitUntil: "networkidle" }).catch(() => {});
  await first.waitForTimeout(3000);
  await first.close();
}
const page = await browser.newPage();
const problems = [];
page.on("console", (message) => {
  if (message.type() === "error" || message.type() === "warning") {
    problems.push(`console.${message.type()}: ${message.text()}`);
  }
});
page.on("pageerror", (error) => problems.push(`page error: ${error.message}`));
const wasm = [];
page.on("response", (response) => {
  if (response.status() >= 400) problems.push(`HTTP ${response.status()}: ${response.url()}`);
  if (response.url().split("?")[0].endsWith(".wasm")) {
    wasm.push(`${response.url()} ${response.headers()["content-type"]}`);
  }
});
const workers = [];
page.on("worker", (worker) => workers.push(worker.url()));

try {
  await page.goto(url, { waitUntil: "load" });
  await page.waitForFunction(() => globalThis.rillStatus?.().loaded, null, { timeout: 30_000 });
  // Highlighted tokens carry Shiki's colors as inline styles.
  const tokens = '[data-testid=history] [data-streamdown="code-block"] pre span[style]';
  await page.waitForSelector(tokens, { timeout: 30_000 });

  // A link in the settled answer opens Streamdown's link safety modal.
  await page.click('[data-testid=history] button[data-streamdown="link"]');
  await page.waitForSelector('[data-streamdown="link-safety-modal"]', { timeout: 10_000 });
  await page.keyboard.press("Escape");
  await page.waitForSelector('[data-streamdown="link-safety-modal"]', { state: "detached", timeout: 10_000 });

  const before = await page.evaluate(() => globalThis.rillStatus().blocks);
  await page.click("[data-testid=stream]");
  await page.waitForSelector("[data-testid=answer][data-done=true]", { timeout: 60_000 });
  await page.waitForFunction(
    () =>
      document.querySelectorAll('[data-testid=answer] [data-streamdown="code-block"]').length === 6 &&
      [...document.querySelectorAll('[data-testid=answer] [data-streamdown="code-block"] pre')].every(
        (pre) => pre.querySelector("span[style]")
      ),
    null,
    { timeout: 30_000 }
  );
  const status = await page.evaluate(() => globalThis.rillStatus());
  if (status.rill === 0) fail(`rill-highlight returned no results: ${JSON.stringify(status)}`);
  // Every streamed block rendered on rill's fast path, none through Streamdown's Block.
  if (status.blocks.streamdown !== before.streamdown || status.blocks.rill <= before.rill) {
    fail(`streamed blocks fell back to Streamdown: ${JSON.stringify({ before, after: status.blocks })}`);
  }
  if (status.shiki > 0) fail(`Shiki highlighted instead of rill-highlight: ${JSON.stringify(status)}`);
  if (workers.length === 0) {
    fail(`no highlighting worker started: ${JSON.stringify(workers)}`);
  }
  const wasmTypes = wasm.filter((line) => !line.endsWith("application/wasm"));
  if (wasm.length === 0) fail("no WebAssembly was fetched");
  if (wasmTypes.length) fail(`WebAssembly served without application/wasm: ${wasmTypes.join(", ")}`);
  // Give late warnings (hydration, effects) a moment.
  await page.waitForTimeout(500);
  if (problems.length) fail(`\n  ${problems.join("\n  ")}`);
  console.log(`smoke: ok ${url} ${JSON.stringify(status)}\n  wasm: ${wasm.join("\n        ")}\n  workers: ${workers.length}`);
} catch (error) {
  const message = error instanceof Failure ? error.message : error.message.split("\n")[0];
  console.error(`smoke: ${url}: ${message}${problems.length ? `\n  ${problems.join("\n  ")}` : ""}`);
  process.exitCode = 1;
} finally {
  await browser.close();
}
