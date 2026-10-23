// Streamdown 2.7.1's `parseMarkdownIntoBlocks` (lib/parse-blocks.tsx; Apache-2.0,
// Copyright 2023 Vercel, Inc.), the same code on the same `marked`, except for what it
// does to the whole text on every streaming update: its footnote and link-definition
// tests look only at what was appended (once either holds it holds for every longer
// text), so an update costs what lexing the tail costs. `conformance/blocks.mjs` checks
// it against Streamdown's on streamed prefixes.
import { Lexer, type Token } from "marked";

const footnoteReferencePattern = /\[\^[\w-]{1,200}\](?!:)/;
const footnoteDefinitionPattern = /\[\^[\w-]{1,200}\]:/;
const openingTagPattern = /<([A-Za-z][\w:-]*)[\s>/]/;
const linkDefinitionPattern = /^ {0,3}\[[^\]]+\]:/m;
/** The longest footnote match: `[^` + 200 + `]:`, plus one for the lookahead. */
const FOOTNOTE_WINDOW = 205;

const voidElements = new Set([
  "area",
  "base",
  "br",
  "col",
  "embed",
  "hr",
  "img",
  "input",
  "link",
  "meta",
  "param",
  "source",
  "track",
  "wbr",
]);

const openTagPatternCache = new Map<string, RegExp>();
const closeTagPatternCache = new Map<string, RegExp>();

const getOpenTagPattern = (tagName: string): RegExp => {
  const normalizedTag = tagName.toLowerCase();
  const cached = openTagPatternCache.get(normalizedTag);
  if (cached) return cached;
  const pattern = new RegExp(`<${normalizedTag}(?=[\\s>/])[^>]*>`, "gi");
  openTagPatternCache.set(normalizedTag, pattern);
  return pattern;
};

const getCloseTagPattern = (tagName: string): RegExp => {
  const normalizedTag = tagName.toLowerCase();
  const cached = closeTagPatternCache.get(normalizedTag);
  if (cached) return cached;
  const pattern = new RegExp(`</${normalizedTag}(?=[\\s>])[^>]*>`, "gi");
  closeTagPatternCache.set(normalizedTag, pattern);
  return pattern;
};

const countNonSelfClosingOpenTags = (block: string, tagName: string): number => {
  if (voidElements.has(tagName.toLowerCase())) return 0;
  const matches = block.match(getOpenTagPattern(tagName));
  if (!matches) return 0;
  let count = 0;
  for (const match of matches) {
    if (!match.trimEnd().endsWith("/>")) count += 1;
  }
  return count;
};

const countClosingTags = (block: string, tagName: string): number => {
  const matches = block.match(getCloseTagPattern(tagName));
  return matches ? matches.length : 0;
};

const countDoubleDollars = (str: string): number => {
  let count = 0;
  for (let i = 0; i < str.length - 1; i += 1) {
    if (str[i] === "$" && str[i + 1] === "$") {
      count += 1;
      i += 1;
    }
  }
  return count;
};

const lineEndingPattern = /\r\n|\r/g;

const lexBlocks = (markdown: string): { tokens: Token[]; hasLinkDefinitions: boolean } => {
  const lexer = new Lexer({ gfm: true });
  const tokens = lexer.blockTokens(markdown);
  return {
    tokens,
    hasLinkDefinitions: Object.keys(lexer.tokens.links).length > 0,
  };
};

interface ParseCache {
  blocks: string[];
  input: string;
  verifiedCount: number;
  verifiedLength: number;
}

const blankLineEnding = "\n\n";

const countStableBlocks = (blocks: string[]): number => {
  for (let i = blocks.length - 3; i >= 0; i -= 1) {
    if ((blocks[i] as string).endsWith(blankLineEnding)) return i + 1;
  }
  return 0;
};

const copyString = (value: string): string => ` ${value}`.slice(1);
const copyBlocks = (blocks: string[]): string[] => blocks.map(copyString);

const mergeTokensIntoBlocks = (tokens: Token[]): string[] => {
  const mergedBlocks: string[] = [];
  const htmlStack: string[] = [];
  let previousTokenWasCode = false;
  for (const token of tokens) {
    const currentBlock = token.raw;
    const mergedBlocksLen = mergedBlocks.length;
    if (htmlStack.length > 0) {
      mergedBlocks[mergedBlocksLen - 1] += currentBlock;
      const trackedTag = htmlStack.at(-1) as string;
      const newOpenTags = countNonSelfClosingOpenTags(currentBlock, trackedTag);
      const newCloseTags = countClosingTags(currentBlock, trackedTag);
      for (let i = 0; i < newOpenTags; i += 1) htmlStack.push(trackedTag);
      for (let i = 0; i < newCloseTags; i += 1) {
        if (htmlStack.length > 0 && htmlStack.at(-1) === trackedTag) htmlStack.pop();
      }
      continue;
    }
    if (token.type === "html" && token.block) {
      const openingTagMatch = currentBlock.match(openingTagPattern);
      if (openingTagMatch) {
        const tagName = openingTagMatch[1] as string;
        const openTags = countNonSelfClosingOpenTags(currentBlock, tagName);
        const closeTags = countClosingTags(currentBlock, tagName);
        if (openTags > closeTags) htmlStack.push(tagName);
      }
    }
    if (token.type === "space" && mergedBlocksLen > 0) {
      mergedBlocks[mergedBlocksLen - 1] += currentBlock;
      continue;
    }
    if (mergedBlocksLen > 0 && !previousTokenWasCode) {
      const previousBlock = mergedBlocks[mergedBlocksLen - 1] as string;
      const prevDollarCount = countDoubleDollars(previousBlock);
      if (prevDollarCount % 2 === 1) {
        mergedBlocks[mergedBlocksLen - 1] = previousBlock + currentBlock;
        continue;
      }
    }
    mergedBlocks.push(currentBlock);
    if (token.type !== "space") previousTokenWasCode = token.type === "code";
  }
  return mergedBlocks;
};

/** `reuseParsedBlocks`, with `linkDefinitions` the answer of its whole-input test. */
const reuseParsedBlocks = (
  previous: ParseCache,
  input: string,
  linkDefinitions: boolean
): ParseCache | null => {
  if (input.length <= previous.input.length) return null;
  if (input.slice(0, previous.input.length) !== previous.input || linkDefinitions) {
    return null;
  }
  const stableCount = countStableBlocks(previous.blocks);
  if (stableCount === 0) return null;
  let verifiedCount = previous.verifiedCount;
  let verifiedLength = previous.verifiedLength;
  if (verifiedCount > stableCount) {
    verifiedCount = stableCount;
    verifiedLength = 0;
    for (let i = 0; i < verifiedCount; i += 1) {
      verifiedLength += (previous.blocks[i] as string).length;
    }
  }
  while (
    verifiedCount < stableCount &&
    input.startsWith(previous.blocks[verifiedCount] as string, verifiedLength)
  ) {
    verifiedLength += (previous.blocks[verifiedCount] as string).length;
    verifiedCount += 1;
  }
  if (verifiedCount !== stableCount) return null;
  const tail = lexBlocks(input.slice(verifiedLength));
  if (tail.hasLinkDefinitions) return null;
  const tailBlocks = copyBlocks(mergeTokensIntoBlocks(tail.tokens));
  return {
    input,
    blocks: previous.blocks.slice(0, stableCount).concat(tailBlocks),
    verifiedCount,
    verifiedLength,
  };
};

let lastParse: ParseCache | null = null;

/** The answers of the whole-text tests for the last text, to extend from. */
let lastTests: { text: string; footnotes: boolean; linkDefinitions: boolean } | null = null;

/** Whether `text` holds a footnote reference or definition, and a link definition. */
function wholeTextTests(text: string): { footnotes: boolean; linkDefinitions: boolean } {
  const previous = lastTests;
  let footnotes: boolean;
  let linkDefinitions: boolean;
  if (previous && text.length >= previous.text.length && text.startsWith(previous.text)) {
    // A match in the old text stays one (a reference that becomes a definition is still a
    // footnote); a new one must reach into the appended text.
    const from = previous.text.length;
    footnotes =
      previous.footnotes ||
      (() => {
        const window = text.slice(Math.max(0, from - FOOTNOTE_WINDOW));
        return footnoteReferencePattern.test(window) || footnoteDefinitionPattern.test(window);
      })();
    // A new definition ends past the old text, and its label holds no `]`, so its `[`
    // comes after the old text's last `]` (bar one that is its final character): test
    // from that line on.
    const bracket = text.lastIndexOf("]", from - 2);
    const line = bracket < 0 ? 0 : text.lastIndexOf("\n", bracket) + 1;
    linkDefinitions = previous.linkDefinitions || linkDefinitionPattern.test(text.slice(line));
  } else {
    footnotes = footnoteReferencePattern.test(text) || footnoteDefinitionPattern.test(text);
    linkDefinitions = linkDefinitionPattern.test(text);
  }
  lastTests = { text, footnotes, linkDefinitions };
  return { footnotes, linkDefinitions };
}

/** Streamdown's `parseMarkdownIntoBlocks`. */
export const parseMarkdownIntoBlocks = (markdown: string): string[] => {
  const tests = wholeTextTests(markdown);
  if (tests.footnotes) return [markdown];
  // The link-definition test reads the normalized input; `\r` changes no match of it
  // (the pattern ends at `]:` and `\r` is not `]`), but the text it extends from must be
  // the same kind of string, so with `\r` it runs on the input.
  const input = markdown.includes("\r") ? markdown.replace(lineEndingPattern, "\n") : markdown;
  if (lastParse?.input === input) return lastParse.blocks;
  const linkDefinitions =
    input === markdown ? tests.linkDefinitions : linkDefinitionPattern.test(input);
  const reused = lastParse ? reuseParsedBlocks(lastParse, input, linkDefinitions) : null;
  let entry = reused;
  if (!entry) {
    const { tokens, hasLinkDefinitions } = lexBlocks(input);
    entry = {
      input,
      blocks: hasLinkDefinitions ? [copyString(input)] : copyBlocks(mergeTokensIntoBlocks(tokens)),
      verifiedCount: 0,
      verifiedLength: 0,
    };
  }
  lastParse = entry;
  return entry.blocks;
};
