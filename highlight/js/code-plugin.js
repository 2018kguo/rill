// A drop-in for @streamdown/code 2.0.0's `createCodePlugin`, highlighting with
// rill-highlight instead of Shiki. The caching and incremental tokenization are
// @streamdown/code's (Apache-2.0, Vercel), with Shiki's `createHighlighter` /
// `codeToTokens` replaced by the rill-highlight core; results are identical.
//
//   const core = await createHighlightCore({ wasm, data });
//   const code = createCodePlugin({ core });
//
// Additions: `highlightSync(options)` returns the result when the language's
// highlighter is already loaded (null otherwise) without a callback round trip.

export function createCodePlugin({ core, themes } = {}) {
  const defaultThemes = themes ?? ['github-light', 'github-dark'];
  const languages = core.languages;
  const languageAliases = {};
  for (const [id, entry] of Object.entries(languages)) for (const alias of entry.aliases ?? []) languageAliases[alias] = id;
  const languageNames = new Set(Object.keys(languages));

  const normalizeLanguage = (language) => {
    const lower = language.trim().toLowerCase();
    return languageAliases[lower] ?? lower;
  };

  const highlighterCache = new Map();
  const loadedHighlighters = new Map();

  const MAX_CACHED_COST = 300_000;
  const MAX_CACHED_RESULTS = 5000;
  const CHARACTERS_PER_COST = 64;
  const tokensCache = new Map();
  let cachedCost = 0;
  const getCost = (key, result) => {
    let cost = result.tokens.length + Math.ceil(key.length / CHARACTERS_PER_COST);
    for (const row of result.tokens) cost += row.length;
    return cost;
  };
  const getCachedTokens = (key) => {
    const entry = tokensCache.get(key);
    if (entry) {
      tokensCache.delete(key);
      tokensCache.set(key, entry);
    }
    return entry?.result;
  };
  const removeCachedTokens = (key) => {
    const entry = tokensCache.get(key);
    if (entry) {
      tokensCache.delete(key);
      cachedCost -= entry.cost;
    }
  };
  const setCachedTokens = (key, result) => {
    removeCachedTokens(key);
    const cost = getCost(key, result);
    tokensCache.set(key, { cost, result });
    cachedCost += cost;
    while ((cachedCost > MAX_CACHED_COST || tokensCache.size > MAX_CACHED_RESULTS) && tokensCache.size > 1) {
      removeCachedTokens(tokensCache.keys().next().value);
    }
  };

  const incrementalStates = new Map();
  const MAX_INCREMENTAL_STATES = 4;
  const takeIncrementalState = (stateKey, code) => {
    const states = incrementalStates.get(stateKey);
    if (!states) return;
    const index = states.findIndex((state) => code.startsWith(state.prefix));
    return index === -1 ? undefined : states.splice(index, 1)[0];
  };
  const storeIncrementalState = (stateKey, state) => {
    let states = incrementalStates.get(stateKey);
    if (!states) {
      states = [];
      incrementalStates.set(stateKey, states);
    }
    states.unshift(state);
    if (states.length > MAX_INCREMENTAL_STATES) states.pop();
  };
  const shiftOffsets = (rows, by) => {
    if (by === 0) return;
    for (const row of rows) for (const token of row) token.offset += by;
  };

  const tokenize = (highlighter, code, stateKey) => {
    const state = takeIncrementalState(stateKey, code);
    let prefix = state ? state.prefix : '';
    let rows = state ? state.rows : [];
    let grammarState = state?.grammarState;
    const lineEnd = code.lastIndexOf('\n') + 1;
    if (lineEnd > prefix.length) {
      const completed = highlighter.codeToTokens(code.slice(prefix.length, lineEnd), { grammarState });
      const newRows = completed.tokens;
      newRows.pop();
      shiftOffsets(newRows, prefix.length);
      rows = rows.concat(newRows);
      grammarState = completed.grammarState;
      prefix = code.slice(0, lineEnd);
      storeIncrementalState(stateKey, { grammarState, prefix, rows });
    } else if (state) {
      storeIncrementalState(stateKey, state);
    }
    const tail = highlighter.codeToTokens(code.slice(lineEnd), { grammarState });
    shiftOffsets(tail.tokens, lineEnd);
    return { ...tail, tokens: rows.concat(tail.tokens) };
  };

  const getThemeName = (theme) => (typeof theme === 'string' ? theme : (theme.name ?? 'custom'));
  const getHighlighterCacheKey = (language, themes) => `${language}-${getThemeName(themes[0])}-${getThemeName(themes[1])}`;
  const getTokensCacheKey = (code, language, themeNames) => `${language}:${themeNames[0]}:${themeNames[1]}\n${code}`;

  const getHighlighter = (language, themes) => {
    const cacheKey = getHighlighterCacheKey(language, themes);
    if (highlighterCache.has(cacheKey)) return highlighterCache.get(cacheKey);
    const promise = core.getHighlighter(language, themes.map(getThemeName)).then((highlighter) => {
      loadedHighlighters.set(cacheKey, highlighter);
      return highlighter;
    });
    highlighterCache.set(cacheKey, promise);
    return promise;
  };

  const resolve = (language, themes) => {
    const resolvedLanguage = normalizeLanguage(language);
    const themeNames = [getThemeName(themes[0]), getThemeName(themes[1])];
    const safeLanguage = languageNames.has(resolvedLanguage) ? resolvedLanguage : 'text';
    return { safeLanguage, themeNames };
  };

  const run = (highlighter, code, safeLanguage, themes, tokensCacheKey, isIncomplete) => {
    let result = getCachedTokens(tokensCacheKey);
    if (!result) {
      result = tokenize(highlighter, code, `${getHighlighterCacheKey(safeLanguage, themes)}:${highlighter.language === 'text' ? 'text' : safeLanguage}`);
      if (!isIncomplete) setCachedTokens(tokensCacheKey, result);
    }
    return result;
  };

  return {
    name: 'shiki',
    type: 'code-highlighter',
    supportsLanguage: (language) => languageNames.has(normalizeLanguage(language)),
    getSupportedLanguages: () => Array.from(languageNames),
    getThemes: () => defaultThemes,
    highlight({ code, language, themes, isIncomplete = false }, callback) {
      const { safeLanguage, themeNames } = resolve(language, themes);
      const tokensCacheKey = getTokensCacheKey(code, safeLanguage, themeNames);
      const cached = getCachedTokens(tokensCacheKey);
      if (cached) return cached;
      getHighlighter(safeLanguage, themes)
        .then((highlighter) => callback?.(run(highlighter, code, safeLanguage, themes, tokensCacheKey, isIncomplete)))
        .catch((error) => console.error('[Streamdown Code] Failed to highlight code:', error));
      return null;
    },
    highlightSync({ code, language, themes, isIncomplete = false }) {
      const { safeLanguage, themeNames } = resolve(language, themes);
      const tokensCacheKey = getTokensCacheKey(code, safeLanguage, themeNames);
      const cached = getCachedTokens(tokensCacheKey);
      if (cached) return cached;
      const highlighter = loadedHighlighters.get(getHighlighterCacheKey(safeLanguage, themes));
      return highlighter ? run(highlighter, code, safeLanguage, themes, tokensCacheKey, isIncomplete) : null;
    },
  };
}
