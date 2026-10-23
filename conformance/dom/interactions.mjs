// Interaction tests for rill's <Streamdown> in Chromium: the controls Streamdown's
// components provide work on rill's DOM (copy and download a code block's code, copy and
// download a table, open a link through the safety modal), and links are plain anchors
// with linkSafety off. Run after dom-diff.mjs --build (it serves conformance/dom/dist).
//
//   node conformance/dom/interactions.mjs [--candidate rill|self]
//
// `--candidate self` runs the same checks on Streamdown itself, which must pass too.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { dirname, extname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const here = dirname(fileURLToPath(import.meta.url));
const dist = join(here, "dist");
const args = process.argv.slice(2);
const candidate = args.includes("--candidate") ? args[args.indexOf("--candidate") + 1] : "rill";
const types = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm" };
const server = createServer(async (request, response) => {
  const path = join(dist, decodeURIComponent(new URL(request.url, "http://x").pathname));
  try {
    const body = await readFile(path);
    response.writeHead(200, { "content-type": types[extname(path)] ?? "application/octet-stream" });
    response.end(body);
  } catch {
    response.writeHead(404);
    response.end();
  }
}).listen(0);
const origin = `http://localhost:${server.address().port}`;

const DOCUMENT = [
  "Read the [guide](https://example.com/guide) first.",
  "",
  "```js",
  "const answer = 42;",
  "console.log(answer);",
  "```",
  "",
  "| Name | Value |",
  "|---|---|",
  "| a | 1 |",
  "| b | 2 |",
].join("\n");

const browser = await chromium.launch();
let failures = 0;
async function check(name, run) {
  const context = await browser.newContext({ permissions: ["clipboard-read", "clipboard-write"], acceptDownloads: true });
  const page = await context.newPage();
  await page.goto(`${origin}/dom.html?config=code&candidate=${candidate}`);
  await page.waitForSelector("body[data-ready]", { state: "attached" });
  try {
    await run(page);
    console.log(`ok   ${name}`);
  } catch (error) {
    failures += 1;
    console.log(`FAIL ${name}: ${error.message}`);
  }
  await context.close();
}
const mount = (page, props = {}) =>
  page.evaluate(([markdown, extra]) => window.mount(markdown, extra), [DOCUMENT, props]);

await check("the code block's copy button copies its code", async (page) => {
  await mount(page);
  await page.click('[data-streamdown="code-block-copy-button"]');
  await page.waitForSelector("output[aria-live]");
  // The fence's text, with its final newline, as Streamdown's button copies it.
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "const answer = 42;\nconsole.log(answer);\n");
});

await check("the code block's download button saves its code", async (page) => {
  await mount(page);
  const download = page.waitForEvent("download");
  await page.click('[data-streamdown="code-block-download-button"]');
  const file = await download;
  assert.match(file.suggestedFilename(), /\.js$/);
  assert.equal(await readFile(await file.path(), "utf8"), "const answer = 42;\nconsole.log(answer);\n");
});

await check("the table's copy menu copies it as CSV", async (page) => {
  await mount(page);
  await page.click('button[title="Copy table"]');
  await page.click('button:text-is("CSV")');
  await page.waitForFunction(async () => (await navigator.clipboard.readText()).includes("Name"));
  assert.equal((await page.evaluate(() => navigator.clipboard.readText())).replace(/\r/g, ""), "Name,Value\na,1\nb,2");
});

await check("the table's download menu saves it as Markdown", async (page) => {
  await mount(page);
  await page.click('button[title="Download table"]');
  const download = page.waitForEvent("download");
  await page.click('button:text-is("Markdown")');
  const file = await download;
  assert.match(await readFile(await file.path(), "utf8"), /\| Name \| Value \|/);
});

await check("a link opens the safety modal, and confirming opens the URL", async (page) => {
  await mount(page);
  await page.evaluate(() => {
    window.opened = [];
    window.open = (...args) => {
      window.opened.push(args);
      return null;
    };
  });
  await page.click('button[data-streamdown="link"]');
  await page.waitForSelector('[data-streamdown="link-safety-modal"]');
  await page.click('button:has-text("Open link")');
  assert.deepEqual(await page.evaluate(() => window.opened), [["https://example.com/guide", "_blank", "noreferrer"]]);
  assert.equal(await page.$('[data-streamdown="link-safety-modal"]'), null);
});

await check("onLinkCheck allowing a link opens it without the modal", async (page) => {
  await page.evaluate(() => {
    window.opened = [];
    window.open = (...args) => {
      window.opened.push(args);
      return null;
    };
    window.mount(
      "See [docs](https://example.com/docs).",
      { linkSafety: { enabled: true, onLinkCheck: (url) => url.startsWith("https://example.com") } }
    );
  });
  await page.click('button[data-streamdown="link"]');
  await page.waitForFunction(() => window.opened.length > 0);
  assert.deepEqual(await page.evaluate(() => window.opened), [["https://example.com/docs", "_blank", "noreferrer"]]);
  assert.equal(await page.$('[data-streamdown="link-safety-modal"]'), null);
});

await check("with linkSafety off, links are anchors", async (page) => {
  await mount(page, { linkSafety: { enabled: false } });
  const link = await page.waitForSelector('a[data-streamdown="link"]');
  assert.equal(await link.getAttribute("href"), "https://example.com/guide");
  assert.equal(await link.getAttribute("target"), "_blank");
  assert.equal(await page.$('button[data-streamdown="link"]'), null);
});

await browser.close();
server.close();
console.log(failures ? `${failures} failed` : "all interactions work");
process.exitCode = failures ? 1 : 0;
