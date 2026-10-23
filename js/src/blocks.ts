// Streamdown 2.7.1's `parseMarkdownIntoBlocks` (lib/parse-blocks.tsx; Apache-2.0,
// Copyright 2023 Vercel, Inc.), the same code on the same `marked`, except for what it
// does to the whole text on every streaming update: its footnote and link-definition
// tests look only at what changed since the last text (a match found before still
// holds while the text keeps it), and it reuses the blocks the last text settled when
// the new one keeps them (Streamdown reuses them only when the text grew by an append,
// but remend's closing markers make a streamed text end differently each time), so an
// update costs what lexing the tail costs. `conformance/blocks.mjs` checks it against
// Streamdown's on streamed prefixes, raw and healed.
import { Lexer, Tokenizer, type Token, type TokensList } from "marked";

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

interface ParseCache {
  /** The input cut into blocks as if it had no link definitions. */
  blocks: string[];
  /** Whether each block holds a link definition (they make the input one block). */
  definitions: boolean[];
  input: string;
  verifiedCount: number;
  verifiedLength: number;
  /** Whether `input[..verifiedLength]` ends inside what could be a link definition's
   * label (see `openLabel`). */
  label: boolean;
}

type Lexed = { blocks: string[]; definitions: boolean[] };

/** How deep `blockTokens` calls are nested, and the depth each running list tokenizer
 * was called at (lists nest through blockquotes and items). */
let lexDepth = 0;
const lists: number[] = [];

/** marked's tokenizer, noting where each list is lexed, and lexing a table's rows no
 * further than to know where it ends. */
class ListTokenizer extends Tokenizer {
  override list(src: string) {
    lists.push(lexDepth);
    try {
      return super.list(src);
    } finally {
      lists.pop();
    }
  }

  /**
   * marked's table, without its rows' cells: only `raw` decides the blocks, and the
   * header and delimiter rows alone decide whether it is a table (their cell counts
   * must match). Splitting and queueing every row's cells made each update of a table
   * that streams cost its whole length again.
   */
  override table(src: string) {
    const rules = (this as unknown as { rules: TokenizerRules }).rules;
    const match = rules.block.table.exec(src);
    if (!match || !rules.other.tableDelimiter.test(match[2]!)) return undefined;
    const head = super.table(`${match[1]}\n${match[2]}\n`);
    if (!head) return undefined;
    let end = match[0].length;
    while (end > 0 && match[0][end - 1] === "\n") end -= 1;
    return { ...head, raw: match[0].slice(0, end), rows: [] };
  }
}

type TokenizerRules = {
  block: { table: RegExp };
  other: { tableDelimiter: RegExp };
};

/**
 * marked's lexer, without the tokens of list items that cannot hold a link definition.
 * A list's `raw` is cut from its lines before its items are lexed, and the items'
 * tokens only decide its looseness and task boxes, so the blocks are the same; only a
 * definition (which needs `]:`) inside an item matters here. Only a list's own calls
 * for its items are skipped: inside an item that is lexed, a blockquote's tokens decide
 * where it ends. Ending as `blockTokens` does, it leaves `state.top` set, which the
 * next paragraph reads.
 */
class BlockLexer extends Lexer {
  override blockTokens(src: string, tokens?: Token[], lastParagraphClipped?: boolean): Token[];
  override blockTokens(src: string, tokens?: TokensList, lastParagraphClipped?: boolean): TokensList;
  override blockTokens(src: string, tokens: Token[] = [], lastParagraphClipped = false) {
    if (lists.length > 0 && lists[lists.length - 1] === lexDepth && !src.includes("]:")) {
      this.state.top = true;
      return tokens;
    }
    lexDepth += 1;
    try {
      return super.blockTokens(src, tokens, lastParagraphClipped);
    } finally {
      lexDepth -= 1;
    }
  }
}

const lexBlocks = (markdown: string): Lexed => {
  lexDepth = 0;
  lists.length = 0;
  const lexer = new BlockLexer({ gfm: true, tokenizer: new ListTokenizer() });
  // Streamdown keeps the text in one block when marked registered a link definition
  // anywhere (its lexer's \`links\`): note which top-level token was being lexed at each
  // registration. (A definition can be registered and then dropped from the tokens: a
  // blockquote's list continuation lexes the list again, and the label is known.)
  const tokens: Token[] = [];
  const defined = new Set<number>();
  const links = lexer.tokens.links;
  lexer.tokens.links = new Proxy(links, {
    set(target, label, value) {
      defined.add(tokens.length);
      return Reflect.set(target, label, value);
    },
  });
  lexer.blockTokens(markdown, tokens);
  const { blocks, definitions } = mergeTokensIntoBlocks(tokens, defined);
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

const mergeTokensIntoBlocks = (tokens: Token[], defined: Set<number>): Lexed => {
  const mergedBlocks: string[] = [];
  const definitions: boolean[] = [];
  const htmlStack: string[] = [];
  let previousTokenWasCode = false;
  for (const [index, token] of tokens.entries()) {
    const currentBlock = token.raw;
    const mergedBlocksLen = mergedBlocks.length;
    const definition = defined.has(index);
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
  let label = previous.label;
  if (verifiedCount > stableCount || verifiedLength > kept) {
    verifiedCount = 0;
    verifiedLength = 0;
    label = false;
  }
  while (
    verifiedCount < stableCount &&
    input.startsWith(previous.blocks[verifiedCount] as string, verifiedLength)
  ) {
    label = openLabel(label, previous.blocks[verifiedCount] as string);
    verifiedLength += (previous.blocks[verifiedCount] as string).length;
    verifiedCount += 1;
  }
  if (verifiedCount !== stableCount) return null;
  // A definition's label may run over blank lines and so start in the kept blocks and
  // end in the tail (the whole text then holds a definition): lex it all when the
  // tail's first bracket closes such a label with `]:`.
  if (label && closesLabel(input, verifiedLength)) return null;
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
    label,
  };
};

const BRACKET_OR_ESCAPE = /[[\]\\]/g;

/**
 * Whether a text that ended inside a possible label (`open`) still does after `text`:
 * marked's definition label (`(?!\s*\])(?:\\[\s\S]|[^\[\]\\])+`) holds no bracket
 * that is not escaped and has no length limit, so an unescaped `[` with no bracket after
 * it may still become one. Conservative: any `[` counts, wherever it is.
 */
/** Whether the first bracket in `text` from `from` (escapes skipped) is a `]` with `:`
 * right after it. */
function closesLabel(text: string, from: number): boolean {
  BRACKET_OR_ESCAPE.lastIndex = from;
  for (let found = BRACKET_OR_ESCAPE.exec(text); found; found = BRACKET_OR_ESCAPE.exec(text)) {
    if (found[0] === "\\") {
      BRACKET_OR_ESCAPE.lastIndex += 1;
      continue;
    }
    return found[0] === "]" && text[found.index + 1] === ":";
  }
  return false;
}

function openLabel(open: boolean, text: string): boolean {
  BRACKET_OR_ESCAPE.lastIndex = 0;
  for (let found = BRACKET_OR_ESCAPE.exec(text); found; found = BRACKET_OR_ESCAPE.exec(text)) {
    if (found[0] === "\\") BRACKET_OR_ESCAPE.lastIndex += 1;
    else open = found[0] === "[";
  }
  return open;
}

/** `input` cut into blocks, from the last cut of a text when it can. */
const cut = (previous: ParseCache | null, input: string): ParseCache => {
  if (previous?.input === input) return previous;
  const reused = previous ? reuseParsedBlocks(previous, input, commonPrefix(previous.input, input)) : null;
  return reused ?? { input, ...lexBlocks(input), verifiedCount: 0, verifiedLength: 0, label: false };
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

let lastText: { text: string; carriageReturn: boolean } | null = null;

/** Whether `text` holds a `\r`, looking past what it keeps of the last text only. */
const hasCarriageReturn = (text: string): boolean => {
  const kept = lastText && !lastText.carriageReturn ? commonPrefix(lastText.text, text) : 0;
  const carriageReturn = (kept === 0 ? text : text.slice(kept)).includes("\r");
  lastText = { text, carriageReturn };
  return carriageReturn;
};

/** The input `parseMarkdownIntoBlocks` last kept in one block for its link definitions. */
let definitionInput: string | null = null;

/** Streamdown's `parseMarkdownIntoBlocks`. */
export const parseMarkdownIntoBlocks = (markdown: string): string[] => {
  if (hasFootnotes(markdown)) return [markdown];
  const input = hasCarriageReturn(markdown)
    ? markdown.replace(lineEndingPattern, "\n")
    : markdown;
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
