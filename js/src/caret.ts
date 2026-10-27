// Streamdown's root asks two questions of the last block on every streamed update, each
// by splitting the whole block into lines: whether it ends inside a code fence (which
// marks the block incomplete and hides the caret) and whether a line is a table's
// delimiter row (which hides the caret too). Only lines that match those patterns change
// either answer. A block that is the whole answer (one with link definitions or
// footnotes, or one long code block) grows with it, so those two passes would cost the
// whole answer on every update. rill hands the root that block as a String whose
// `split("\n")` gives only the matching lines, in order, kept as the text grows: the
// same two answers from the new lines alone.

import { commonPrefix } from "./prefix.ts";

/** Streamdown's patterns, as its root tests them (`streamdown` 2.7.1). */
const FENCE = /^[ \t]{0,3}(`{3,}|~{3,})/;
const DELIMITER_ROW = /^\|?[ \t]*:?-{1,}:?[ \t]*(\|[ \t]*:?-{1,}:?[ \t]*)*\|?$/;

/** Whether a line can change either answer. A delimiter row holds only `|-: \t`, so the
 * two kinds never overlap: each test ignores the other's lines. */
const matters = (line: string): boolean => {
  if (FENCE.test(line)) return true;
  const trimmed = line.trim();
  return trimmed.length > 0 && trimmed.includes("|") && DELIMITER_ROW.test(trimmed);
};

type Scan = {
  text: string;
  /** Where the lines scanned so far end: just after a `\n`, or 0. */
  end: number;
  /** The matching lines before `end`, with where each starts. */
  lines: { at: number; line: string }[];
};

let last: Scan | null = null;

/** Below this, splitting the block is cheaper than keeping its lines. */
export const CARET_VIEW_MIN = 2048;

/** `text` as a String whose `split("\n")` holds only the lines that matter. */
export function caretView(text: string): string {
  let scan: Scan;
  if (last) {
    // The lines that end before the first change stay as they were.
    const kept = commonPrefix(last.text, text);
    const from = text.lastIndexOf("\n", kept - 1) + 1;
    const { lines } = last;
    while (lines.length > 0 && lines[lines.length - 1]!.at >= from) lines.pop();
    scan = { text, end: from, lines };
  } else {
    scan = { text, end: 0, lines: [] };
  }
  const end = text.lastIndexOf("\n") + 1;
  for (let at = scan.end; at < end; ) {
    const next = text.indexOf("\n", at);
    const line = text.slice(at, next);
    if (matters(line)) scan.lines.push({ at, line });
    at = next + 1;
  }
  scan.end = end;
  last = scan;
  const lines = scan.lines.map(({ line }) => line);
  const tail = text.slice(scan.end);
  if (matters(tail)) lines.push(tail);
  const view = new String(text) as String & { split: typeof String.prototype.split };
  view.split = ((separator: string | RegExp, limit?: number) =>
    separator === "\n" && limit === undefined
      ? lines.slice()
      : text.split(separator as string, limit)) as typeof String.prototype.split;
  return view as unknown as string;
}
