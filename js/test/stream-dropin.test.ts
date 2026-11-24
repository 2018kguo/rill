// The drop-in in Streamdown's default setup streams through rill-stream: after every
// update its DOM is Streamdown's, and a click on a button hands the message to
// Streamdown's components and lands on the same button there.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import { mount } from "./dom.ts";

const { Streamdown } = await import("streamdown");
const { initStream } = await import("../src/stream.ts");
const { Streamdown: RillStreamdown, blockRenders } = await import("../src/streamdown.ts");
const { Streamdown: LightStreamdown } = await import("../src/stream-entry.ts");

await initStream(readFileSync(new URL("../wasm/rill_stream_bg.wasm", import.meta.url)));

const ANSWER = [
  "# Plan\n\nWe **retry** only *idempotent* requests, see [the RFC](https://example.com/rfc).\n\n",
  "- [x] Full jitter\n- [ ] Metrics per attempt\n\n",
  "| status | retry |\n|:--|--:|\n| 503 | yes |\n| 400 | ~~no~~ |\n\n",
  "```ts\nconst delay = base * 2 ** attempt;\n```\n\n",
  "> Done when the budget runs out: https://example.org/budget.\n",
].join("");

test("streams Streamdown's DOM at every update, without React per block", async () => {
  const ours = mount(RillStreamdown);
  const theirs = mount(Streamdown);
  const before = blockRenders.rill + blockRenders.streamdown;
  for (let end = 1; end <= ANSWER.length; end += 7) {
    const text = ANSWER.slice(0, end);
    await ours.render(text, { isAnimating: true });
    await theirs.render(text, { isAnimating: true });
    assert.equal(ours.host.innerHTML, theirs.host.innerHTML, JSON.stringify(text));
  }
  await ours.render(ANSWER, { isAnimating: false });
  await theirs.render(ANSWER, { isAnimating: false });
  assert.equal(ours.host.innerHTML, theirs.host.innerHTML);
  assert.equal(blockRenders.rill + blockRenders.streamdown, before, "no Block rendered: the streaming root did");
  await ours.unmount();
  await theirs.unmount();
});

test("a text with footnotes and link definitions streams as one block", async () => {
  const text = [
    "# Notes\n\nRetries[^jitter] follow [the guide][guide] and [policy].\n\n",
    "- [x] Done[^jitter]\n\n[^jitter]: Full jitter, see [policy].\n\n    Second paragraph.\n\n",
    "[guide]: https://example.com/guide \"Guide\"\n[policy]: https://example.com/policy\n\n",
    "```ts\nconst a = 1;\n```\n\nDone.\n",
  ].join("");
  const ours = mount(RillStreamdown);
  const theirs = mount(Streamdown);
  const before = blockRenders.rill + blockRenders.streamdown;
  for (let end = 1; end <= text.length; end += 3) {
    await ours.render(text.slice(0, end), { isAnimating: true });
    await theirs.render(text.slice(0, end), { isAnimating: true });
    assert.equal(ours.host.innerHTML, theirs.host.innerHTML, JSON.stringify(text.slice(0, end)));
  }
  await ours.render(text, { isAnimating: false });
  await theirs.render(text, { isAnimating: false });
  assert.equal(ours.host.innerHTML, theirs.host.innerHTML);
  assert.equal(blockRenders.rill + blockRenders.streamdown, before);
  await ours.unmount();
  await theirs.unmount();
});

test("the benchmark's props (dir, no line numbers, no code height) stream too", async () => {
  const props = { mode: "streaming", dir: "ltr", controls: false, lineNumbers: false, codeBlockMaxHeight: 0 };
  const ours = mount(RillStreamdown);
  const theirs = mount(Streamdown);
  const before = blockRenders.rill + blockRenders.streamdown;
  for (let end = 1; end <= ANSWER.length; end += 5) {
    const text = ANSWER.slice(0, end);
    await ours.render(text, { ...props, isAnimating: true });
    await theirs.render(text, { ...props, isAnimating: true });
    assert.equal(ours.host.innerHTML, theirs.host.innerHTML, JSON.stringify(text));
  }
  assert.equal(blockRenders.rill + blockRenders.streamdown, before);
  await ours.unmount();
  await theirs.unmount();
});

test("the light entry streams the same DOM", async () => {
  const ours = mount(LightStreamdown);
  const theirs = mount(Streamdown);
  for (let end = 3; end <= ANSWER.length; end += 11) {
    const text = ANSWER.slice(0, end);
    await ours.render(text, { isAnimating: true });
    await theirs.render(text, { isAnimating: true });
    assert.equal(ours.host.innerHTML, theirs.host.innerHTML, JSON.stringify(text));
  }
  await ours.unmount();
  await theirs.unmount();
});

test("the light entry merges a className as Streamdown does", async () => {
  const props = { className: "mt-2 space-y-2", isAnimating: true };
  const ours = mount(LightStreamdown);
  const theirs = mount(Streamdown);
  await ours.render(ANSWER.slice(0, 40), props);
  // (The merge loads on first use.)
  await new Promise((resolve) => setTimeout(resolve, 50));
  await ours.render(ANSWER.slice(0, 60), props);
  await theirs.render(ANSWER.slice(0, 60), props);
  assert.equal(ours.host.innerHTML, theirs.host.innerHTML);
  assert.match(ours.host.innerHTML, /space-y-2/);
  await ours.unmount();
  await theirs.unmount();
});

test("a click hands the message to Streamdown and reaches the same button", async () => {
  const opened = (host: HTMLElement) => {
    host.querySelector<HTMLElement>('button[data-streamdown="link"]')!.click();
    return document.body.innerHTML.includes("example.com/rfc");
  };
  const ours = mount(RillStreamdown);
  await ours.render(ANSWER);
  const before = blockRenders.rill + blockRenders.streamdown;
  const { act } = await import("react");
  let shown = false;
  await act(async () => {
    shown = opened(ours.host);
  });
  assert.ok(blockRenders.rill + blockRenders.streamdown > before, "handed off to Streamdown's components");
  // Streamdown's link-safety dialog names the URL.
  assert.ok(shown || document.body.innerHTML.includes("example.com/rfc"), "the link's dialog opened");
  await ours.unmount();
});
