// Streamdown 2.7.1's `parseMarkdownIntoBlocks` (lib/parse-blocks.tsx; Apache-2.0,
// Copyright 2023 Vercel, Inc.), the same code on the same `marked`, except for what it
// does to the whole text on every streaming update: its footnote and link-definition
// tests look only at what changed since the last text (a match found before still
// holds while the text keeps it), and it reuses the blocks the last text settled when
// the new one keeps them (Streamdown reuses them only when the text grew by an append,
// but remend's closing markers make a streamed text end differently each time), so an
// update costs what lexing the tail costs. `conformance/blocks.mjs` checks it against
// Streamdown's on streamed prefixes, raw and healed.
import { Lexer, type Token } from "marked";

import { commonPrefix, Lru } from "./prefix.ts";

const footnoteReferencePattern = /\[\^[\w-]{1,200}\](?!:)/;
const footnoteDefinitionPattern = /\[\^[\w-]{1,200}\]:/;
const openingTagPattern = /<([A-Za-z][\w:-]*)[\s>/]/;
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

/** Whether marked found a link definition in `token` (it keeps the first of each label
 * as a `def` token, wherever it is). */
const hasDefinition = (token: Token): boolean =>
  token.type === "def" ||
  Boolean((token as { tokens?: Token[] }).tokens?.some(hasDefinition)) ||
  Boolean((token as { items?: { tokens?: Token[] }[] }).items?.some((item) => item.tokens?.some(hasDefinition)));

interface ParseCache {
  /** The input cut into blocks as if it had no link definitions. */
  blocks: string[];
  /** Whether each block holds a link definition (they make the input one block). */
  definitions: boolean[];
  input: string;
  verifiedCount: number;
  verifiedLength: number;
}

type Lexed = { blocks: string[]; definitions: boolean[] };

const lexBlocks = (markdown: string): Lexed => {
  const tokens = new Lexer({ gfm: true }).blockTokens(markdown);
  const { blocks, definitions } = mergeTokensIntoBlocks(tokens);
  return { blocks: copyBlocks(blocks), definitions };
};

const blankLineEnding = "\n\n";

const countStableBlocks = (blocks: string[]): number => {
  for (let i = blocks.length - 3; i >= 0; i -= 1) {
    if ((blocks[i] as string).endsWith(blankLineEnding)) return i + 1;
  }
  return 0;
};

const copyString = (value: string): string => ` ${value}`.slice(1);
const copyBlocks = (blocks: string[]): string[] => blocks.map(copyString);

const mergeTokensIntoBlocks = (tokens: Token[]): Lexed => {
  const mergedBlocks: string[] = [];
  const definitions: boolean[] = [];
  const htmlStack: string[] = [];
  let previousTokenWasCode = false;
  for (const token of tokens) {
    const currentBlock = token.raw;
    const mergedBlocksLen = mergedBlocks.length;
    const definition = hasDefinition(token);
    if (htmlStack.length > 0) {
      mergedBlocks[mergedBlocksLen - 1] += currentBlock;
      definitions[mergedBlocksLen - 1] ||= definition;
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
        definitions[mergedBlocksLen - 1] ||= definition;
        continue;
      }
    }
    mergedBlocks.push(currentBlock);
    definitions.push(definition);
    if (token.type !== "space") previousTokenWasCode = token.type === "code";
  }
  return { blocks: mergedBlocks, definitions };
};

/** `reuseParsedBlocks` for an input that keeps the last one's first `kept` units. */
const reuseParsedBlocks = (
  previous: ParseCache,
  input: string,
  kept: number
): ParseCache | null => {
  const stableCount = countStableBlocks(previous.blocks);
  if (stableCount === 0) return null;
  let verifiedCount = previous.verifiedCount;
  let verifiedLength = previous.verifiedLength;
  if (verifiedCount > stableCount || verifiedLength > kept) {
    verifiedCount = 0;
    verifiedLength = 0;
  }
  while (
    verifiedCount < stableCount &&
    input.startsWith(previous.blocks[verifiedCount] as string, verifiedLength)
  ) {
    verifiedLength += (previous.blocks[verifiedCount] as string).length;
    verifiedCount += 1;
  }
  if (verifiedCount !== stableCount) return null;
  // A block's end (its trailing blank lines) depends on the text up to the next one: the
  // text must keep some of that.
  if (!/\S/.test(input.slice(verifiedLength, kept))) return null;
  const tail = lexBlocks(input.slice(verifiedLength));
  return {
    input,
    blocks: previous.blocks.slice(0, stableCount).concat(tail.blocks),
    definitions: previous.definitions.slice(0, stableCount).concat(tail.definitions),
    verifiedCount,
    verifiedLength,
  };
};

/** `input` cut into blocks, from the last cut of a text when it can. */
const cut = (previous: ParseCache | null, input: string): ParseCache => {
  if (previous?.input === input) return previous;
  const reused = previous ? reuseParsedBlocks(previous, input, commonPrefix(previous.input, input)) : null;
  return reused ?? { input, ...lexBlocks(input), verifiedCount: 0, verifiedLength: 0 };
};

let lastParse: ParseCache | null = null;
/** Texts cut before, for messages that do not stream (mounted again, or repeated). */
const cuts = new Lru<string, ParseCache>(64);

/** Where the first footnote match in `text` from `from` ends (with its lookahead), or
 * -1. */
const firstFootnoteEnd = (text: string, from: number): number => {
  const rest = from === 0 ? text : text.slice(from);
  const ends: number[] = [];
  const reference = footnoteReferencePattern.exec(rest);
  if (reference) ends.push(from + reference.index + reference[0].length + 1);
  const definition = footnoteDefinitionPattern.exec(rest);
  if (definition) ends.push(from + definition.index + definition[0].length);
  return ends.length > 0 ? Math.min(...ends) : -1;
};

/** Whether a text holds a footnote reference or definition, extending the answer for
 * the last text it was asked about. */
const footnoteTest = (): ((text: string) => boolean) => {
  let last: { text: string; end: number } | null = null;
  return (text) => {
    let end: number;
    if (last) {
      // A match within what the text keeps of the last one still holds; a new match must
      // reach past it, and is at most `FOOTNOTE_WINDOW` long.
      const kept = commonPrefix(last.text, text);
      end =
        last.end >= 0 && last.end <= kept
          ? last.end
          : firstFootnoteEnd(text, Math.max(0, kept - FOOTNOTE_WINDOW));
    } else {
      end = firstFootnoteEnd(text, 0);
    }
    last = { text, end };
    return end >= 0;
  };
};

const hasFootnotes = footnoteTest();

/** The input `parseMarkdownIntoBlocks` last kept in one block for its link definitions. */
let definitionInput: string | null = null;

/** Streamdown's `parseMarkdownIntoBlocks`. */
export const parseMarkdownIntoBlocks = (markdown: string): string[] => {
  if (hasFootnotes(markdown)) return [markdown];
  const input = markdown.includes("\r") ? markdown.replace(lineEndingPattern, "\n") : markdown;
  // A text unlike the last one (another message) may have been cut before.
  const unlike = !lastParse || commonPrefix(lastParse.input, input) < input.length >> 1;
  const known = unlike ? cuts.get(input) : undefined;
  lastParse = known ?? cut(lastParse, input);
  if (unlike && !known) cuts.set(input, lastParse);
  // Each block is its own Markdown document, so a text with link definitions stays one.
  if (lastParse.definitions.some(Boolean)) {
    definitionInput = input;
    return [input];
  }
  return lastParse.blocks;
};

let lastChunks: ParseCache | null = null;

/**
 * The pieces of a text `parseMarkdownIntoBlocks` kept in one block for its link
 * definitions: the blocks it would have made without them, or `null` for a text it did
 * not keep so (or one marked did not cut into pieces of itself: it drops a repeated
 * definition and rewrites some blockquotes). rill renders the pieces one by one, with
 * the text's definitions, and reuses all but the last as the text grows.
 */
export const splitDefinitionBlock = (input: string): string[] | null => {
  if (input !== definitionInput) return null;
  const parse = lastParse?.input === input ? lastParse : (lastChunks = cut(lastChunks, input));
  const { blocks, verifiedCount } = parse;
  // The reused blocks were compared with the text already.
  let offset = parse.verifiedLength;
  for (let index = verifiedCount; index < blocks.length; index += 1) {
    const block = blocks[index] as string;
    if (!input.startsWith(block, offset)) return null;
    offset += block.length;
  }
  return offset === input.length ? blocks : null;
};
