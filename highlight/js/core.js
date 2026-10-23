// rill-highlight's JavaScript core: loads the wasm module and the grammar/theme
// data, and returns Shiki-shaped `codeToTokens` results.
//
//   const core = await createHighlightCore({ wasm, data });
//   const highlighter = await core.getHighlighter('typescript', ['github-light', 'github-dark']);
//   highlighter.codeToTokens(code, { grammarState });
//
// `wasm`: a WebAssembly.Module, ArrayBuffer/Uint8Array, Response, or URL.
// `data`: { languages: object (dist/languages.json),
//           grammar(name): Promise<string>, theme(name): Promise<string> }.

const PLAIN_LANGUAGES = new Set(['plaintext', 'txt', 'text', 'plain']);

async function instantiate(wasm) {
  if (wasm instanceof WebAssembly.Module) return WebAssembly.instantiate(wasm, {});
  if (typeof Response !== 'undefined' && wasm instanceof Response) {
    return (await WebAssembly.instantiate(await wasm.arrayBuffer(), {})).instance;
  }
  if (wasm instanceof URL || typeof wasm === 'string') {
    return (await WebAssembly.instantiate(await (await fetch(wasm)).arrayBuffer(), {})).instance;
  }
  return (await WebAssembly.instantiate(wasm, {})).instance;
}

export async function createHighlightCore({ wasm, data }) {
  const instance = await instantiate(wasm);
  const x = instance.exports;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const text = (length) => decoder.decode(new Uint8Array(x.memory.buffer, x.rhl_text(), length));
  const error = () => new Error(`[rill-highlight] ${text(x.rhl_text_len())}`);
  const withBytes = (string, use) => {
    const bytes = encoder.encode(string);
    const ptr = x.rhl_alloc(bytes.length);
    new Uint8Array(x.memory.buffer, ptr, bytes.length).set(bytes);
    try {
      return use(ptr, bytes.length);
    } finally {
      x.rhl_free(ptr, bytes.length);
    }
  };
  // Grammar states live in wasm memory until their JS wrapper is collected.
  const states = typeof FinalizationRegistry === 'undefined' ? null : new FinalizationRegistry((handle) => x.rhl_state_free(handle));

  const themeHandles = new Map();
  const loadTheme = (name) => {
    if (!themeHandles.has(name)) {
      themeHandles.set(name, Promise.resolve(data.theme(name)).then((json) => withBytes(json, (p, n) => x.rhl_add_theme(p, n))));
    }
    return themeHandles.get(name);
  };
  const grammarTexts = new Map();
  const loadGrammar = (name) => {
    if (!grammarTexts.has(name)) grammarTexts.set(name, Promise.resolve(data.grammar(name)));
    return grammarTexts.get(name);
  };

  /** A highlighter for one language and a [light, dark] theme pair. */
  async function getHighlighter(language, themeNames) {
    const [light, dark] = await Promise.all(themeNames.map(loadTheme));
    if (PLAIN_LANGUAGES.has(language)) return plainHighlighter(themeNames);
    const entry = data.languages[language];
    if (!entry) throw new Error(`[rill-highlight] unknown language ${language}`);
    const grammars = await Promise.all(entry.grammars.map(loadGrammar));
    for (const json of grammars) withBytes(json, (p, n) => x.rhl_add_grammar(p, n));
    const handle = withBytes(entry.name, (p, n) => x.rhl_highlighter_new(p, n, light, dark));
    if (!handle) throw error();
    const [fg, bg, themeName] = JSON.parse(text(x.rhl_colors(handle)));
    const styles = [];
    let codeBuffer = { ptr: 0, units: 0 };

    function codeToTokens(code, { grammarState } = {}) {
      if (codeBuffer.units < code.length) {
        if (codeBuffer.ptr) x.rhl_free(codeBuffer.ptr, codeBuffer.units * 2);
        const units = Math.max(code.length, codeBuffer.units * 2, 1024);
        codeBuffer = { ptr: x.rhl_alloc(units * 2), units };
      }
      const units = new Uint16Array(x.memory.buffer, codeBuffer.ptr, code.length);
      for (let i = 0; i < code.length; i++) units[i] = code.charCodeAt(i);
      const out = x.rhl_tokenize(handle, codeBuffer.ptr, code.length, grammarState ? grammarState.handle : 0);
      if (!out) throw error();
      const view = new Uint32Array(x.memory.buffer, out);
      const stateHandle = view[0];
      const lineCount = view[1];
      // Read the numbers first: creating style objects may call back into wasm.
      let at = 2;
      let maxStyle = -1;
      const numbers = [];
      for (let line = 0; line < lineCount; line++) {
        const count = view[at++];
        numbers.push(count);
        for (let t = 0; t < count; t++) {
          numbers.push(view[at], view[at + 1], view[at + 2]);
          if (view[at + 2] > maxStyle) maxStyle = view[at + 2];
          at += 3;
        }
      }
      if (maxStyle >= styles.length) {
        for (const pairs of JSON.parse(text(x.rhl_styles(handle, styles.length)))) styles.push(Object.fromEntries(pairs));
      }
      const tokens = [];
      let k = 0;
      for (let line = 0; line < lineCount; line++) {
        const count = numbers[k++];
        const row = new Array(count);
        for (let t = 0; t < count; t++) {
          const offset = numbers[k], length = numbers[k + 1], style = numbers[k + 2];
          k += 3;
          row[t] = { content: code.slice(offset, offset + length), explanation: undefined, offset, htmlStyle: { ...styles[style] } };
        }
        tokens.push(row);
      }
      const state = { handle: stateHandle, lang: entry.name };
      if (stateHandle && states) states.register(state, stateHandle);
      return { tokens, fg, bg, themeName, rootStyle: undefined, grammarState: state };
    }

    return { language: entry.name, codeToTokens, dispose: () => x.rhl_highlighter_free(handle) };
  }

  /** Shiki's result for plain-text languages: one unstyled token per line. */
  async function plainHighlighter(themeNames) {
    const colors = await getColors(themeNames);
    function codeToTokens(code) {
      const tokens = [];
      let offset = 0;
      for (const line of code.split(/\r?\n/)) {
        tokens.push([{ content: line, explanation: undefined, offset, htmlStyle: {} }]);
        offset += line.length;
        offset += code.startsWith('\r\n', offset) ? 2 : 1;
      }
      return { tokens, ...colors, rootStyle: undefined, grammarState: undefined };
    }
    return { language: 'text', codeToTokens, dispose() {} };
  }

  // The fg/bg/themeName of a theme pair, from a throwaway highlighter.
  async function getColors(themeNames) {
    const [l, d] = await Promise.all(themeNames.map(loadTheme));
    const handle = withBytes('text.plain.rill', (p, n) => {
      withBytes(JSON.stringify({ grammar: { name: 'text.plain.rill', scopeName: 'text.plain.rill', patterns: [] }, regexes: { '': null, '￿': null } }), (q, m) => x.rhl_add_grammar(q, m));
      return x.rhl_highlighter_new(p, n, l, d);
    });
    if (!handle) throw error();
    const [fg, bg, themeName] = JSON.parse(text(x.rhl_colors(handle)));
    x.rhl_highlighter_free(handle);
    return { fg, bg, themeName };
  }

  return { getHighlighter, languages: data.languages };
}
