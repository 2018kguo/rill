// Browser bench: @streamdown/code (Shiki) vs rill-highlight's plugin in Chromium
// with 10x CPU throttling, on AI-answer-sized code blocks.
//   node browser/run.mjs            (after `node browser/build.mjs`)
// history: highlight 16 finished blocks from a cold page (includes loading).
// warm: highlight the same blocks again (changed, so not cached) once loaded.
// stream: stream each block in 12-character chunks (isIncomplete), then finish it.
import { chromium } from 'playwright-core';
import { serve } from './serve.mjs';
const executablePath = process.env.CHROME ?? `${process.env.HOME}/Library/Caches/ms-playwright/chromium-1248/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing`;
const throttle = Number(process.env.THROTTLE ?? 10);
const runs = Number(process.env.RUNS ?? 3);
const server = await serve();
const base = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch({ executablePath });
const rows = [];
for (const scenario of ['history', 'warm', 'stream']) {
  const row = { scenario };
  for (const engine of ['shiki', 'rill']) {
    let best = Infinity, tokens = 0, setup = 0;
    for (let i = 0; i < runs; i++) {
      const context = await browser.newContext();
      const page = await context.newPage();
      const cdp = await context.newCDPSession(page);
      await cdp.send('Emulation.setCPUThrottlingRate', { rate: throttle });
      await page.goto(`${base}/browser/bench.html?engine=${engine}&scenario=${scenario}`);
      const result = await page.waitForFunction(() => window.__result, null, { timeout: 600_000 }).then((h) => h.jsonValue());
      if (result.ms < best) setup = result.setupMs;
      best = Math.min(best, result.ms);
      tokens = result.tokens;
      await context.close();
    }
    row[`${engine}Ms`] = Math.round(best);
    row[`${engine}SetupMs`] = Math.round(setup);
  }
  row.speedup = +(row.shikiMs / row.rillMs).toFixed(1);
  rows.push(row);
  console.error(JSON.stringify(row));
}
console.table(rows);
await browser.close();
server.close();
