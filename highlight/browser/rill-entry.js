import { createHighlightCore } from '../js/core.js';
import { createCodePlugin } from '../js/code-plugin.js';
window.__createPlugin = async () => {
  const languages = await (await fetch('/dist/languages.json')).json();
  const core = await createHighlightCore({
    wasm: await fetch('/dist/rill_highlight.wasm'),
    data: {
      languages,
      grammar: (name) => fetch(`/dist/grammars/${name}.json`).then((r) => r.text()),
      theme: (name) => fetch(`/dist/themes/${name}.json`).then((r) => r.text()),
    },
  });
  return createCodePlugin({ core });
};
