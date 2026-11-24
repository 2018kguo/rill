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
const { loadIslands } = await import("../src/stream.ts");
await loadIslands();

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
  assert.equal(
    blockRenders.rill + blockRenders.streamdown,
    before,
    "no Block rendered: the streaming root did",
  );
  await ours.unmount();
  await theirs.unmount();
});

test("a text with footnotes and link definitions streams as one block", async () => {
  const text = [
    "# Notes\n\nRetries[^jitter] follow [the guide][guide] and [policy].\n\n",
    "- [x] Done[^jitter]\n\n[^jitter]: Full jitter, see [policy].\n\n    Second paragraph.\n\n",
    '[guide]: https://example.com/guide "Guide"\n[policy]: https://example.com/policy\n\n',
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
  const props = {
    mode: "streaming",
    dir: "ltr",
    controls: false,
    lineNumbers: false,
    codeBlockMaxHeight: 0,
  };
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

test("hydrates the server's HTML with the stream module already loaded", async () => {
  const { renderToString } = await import("react-dom/server");
  const { hydrateRoot } = await import("react-dom/client");
  const { act, createElement } = await import("react");
  const element = createElement(RillStreamdown, {
    isAnimating: true,
    children: ANSWER.slice(0, 80),
  });
  // (The server snapshot is 0, so the server renders Streamdown's tree as one without
  // the module does, and the hydrating render must match it.)
  const host = document.createElement("div");
  host.innerHTML = renderToString(element);
  document.body.append(host);
  const errors: unknown[] = [];
  let root: ReturnType<typeof hydrateRoot> | undefined;
  await act(async () => {
    root = hydrateRoot(host, element, { onRecoverableError: (error) => errors.push(error) });
  });
  assert.deepEqual(errors, []);
  await act(async () => root!.unmount());
  host.remove();
});

// A page's own components, printing every prop they get: the stream renders them as
// islands, with the same props and children as Streamdown.
const shown = (tag: string) =>
  function Shown(props: Record<string, unknown>) {
    const { node, children, ...rest } = props as { node: { tagName: string }; children: unknown };
    return createElement(
      tag,
      { "data-props": JSON.stringify(rest), "data-node": node.tagName, className: "page" },
      children as never,
    );
  };
const { createElement } = await import("react");
const COMPONENTS = { a: shown("a"), table: shown("table"), span: shown("span") };
const ISLANDS = [
  '# Links\n\nSee [the guide](https://example.com/g "Guide") and **[bold](/rel)**, <https://b.org>, www.c.com.\n\n',
  "A [blocked](javascript:x) one, `code` [in *a* link](https://x.com/a_(b)) too.\n\n",
  "| a | [b](https://x.com) |\n|:-|-:|\n| 1 | **2** |\n| [3](/three) | 4 |\n\n",
  "Then [an incomplete link](https://exa",
].join("");
// (A link definition keeps the text one block: its own test below.)
const DEFINED =
  "A [blocked](javascript:x) and [ref][r] too.\n\n[r]: https://example.com/r 'Ref'\n\nAfter [ref].";

test("a page's own components stream as islands, with Streamdown's props and DOM", async () => {
  const ours = mount(RillStreamdown);
  const theirs = mount(Streamdown);
  const before = blockRenders.rill + blockRenders.streamdown;
  const strip = (html: string) => html.replaceAll("<!---->", "");
  for (let end = 1; end <= ISLANDS.length; end += 3) {
    const text = ISLANDS.slice(0, end);
    await ours.render(text, { isAnimating: true, components: COMPONENTS });
    await theirs.render(text, { isAnimating: true, components: COMPONENTS });
    assert.equal(strip(ours.host.innerHTML), theirs.host.innerHTML, JSON.stringify(text));
  }
  await ours.render(ISLANDS, { isAnimating: false, components: COMPONENTS });
  await theirs.render(ISLANDS, { isAnimating: false, components: COMPONENTS });
  assert.equal(strip(ours.host.innerHTML), theirs.host.innerHTML);
  assert.equal(blockRenders.rill + blockRenders.streamdown, before, "streamed, no Block rendered");
  await ours.unmount();
  await theirs.unmount();
});

test("islands: random texts stream as Streamdown renders them, at every prefix", async () => {
  const pieces = [
    "[a](https://x.com)",
    "[b](javascript:y)",
    '[c](/p "T")',
    "**",
    "*",
    "`c`",
    " ",
    "\n\n",
    "\n",
    "| a | [b](https://t.co) |\n|-|-|\n| 1 | 2 |\n",
    "https://w.org",
    "<br>",
    "# h\n",
    "- i ",
    "> q ",
    "```\nc\n```\n",
    "[",
    "](",
    ")",
    "x",
  ];
  let seed = 7;
  const random = (n: number) => ((seed = (seed * 1103515245 + 12345) % 2 ** 31), seed % n);
  const ours = mount(RillStreamdown);
  const theirs = mount(Streamdown);
  for (let round = 0; round < Number(process.env.ISLAND_ROUNDS ?? 40); round += 1) {
    let text = "";
    for (let count = 2 + random(8); count > 0; count -= 1) text += pieces[random(pieces.length)];
    for (let end = 1; end <= text.length; end += 1) {
      const prefix = text.slice(0, end);
      await ours.render(prefix, { isAnimating: true, components: COMPONENTS });
      await theirs.render(prefix, { isAnimating: true, components: COMPONENTS });
      assert.equal(
        ours.host.innerHTML.replaceAll("<!---->", ""),
        theirs.host.innerHTML,
        JSON.stringify(prefix),
      );
    }
    await ours.render(text, { isAnimating: false, components: COMPONENTS });
    await theirs.render(text, { isAnimating: false, components: COMPONENTS });
    assert.equal(
      ours.host.innerHTML.replaceAll("<!---->", ""),
      theirs.host.innerHTML,
      JSON.stringify(text),
    );
    await ours.render("", { isAnimating: true, components: COMPONENTS });
    await theirs.render("", { isAnimating: true, components: COMPONENTS });
  }
  await ours.unmount();
  await theirs.unmount();
});

test("islands in a text kept whole (a link definition): references get the definition's title", async () => {
  const ours = mount(RillStreamdown);
  const theirs = mount(Streamdown);
  const before = blockRenders.rill + blockRenders.streamdown;
  for (let end = 1; end <= DEFINED.length; end += 2) {
    const text = DEFINED.slice(0, end);
    await ours.render(text, { isAnimating: true, components: COMPONENTS });
    await theirs.render(text, { isAnimating: true, components: COMPONENTS });
    assert.equal(
      ours.host.innerHTML.replaceAll("<!---->", ""),
      theirs.host.innerHTML,
      JSON.stringify(text),
    );
  }
  assert.equal(blockRenders.rill + blockRenders.streamdown, before);
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
  assert.ok(
    blockRenders.rill + blockRenders.streamdown > before,
    "handed off to Streamdown's components",
  );
  // Streamdown's link-safety dialog names the URL.
  assert.ok(
    shown || document.body.innerHTML.includes("example.com/rfc"),
    "the link's dialog opened",
  );
  await ours.unmount();
});
