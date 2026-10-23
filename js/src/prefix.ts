// How much of a text the next one keeps: the length of a common prefix of two
// strings, in UTF-16 units. rill's remend reports what it kept of the text it healed
// before (`noteKept`), so a streaming update finds it without comparing the texts.

let kept: { before: string; after: string; length: number } | null = null;

/** `after` starts with `before`'s first `length` units. */
export function noteKept(before: string, after: string, length: number): void {
  kept = { before, after, length };
}

/** A length both texts start with: the longest one, or what `noteKept` said. */
export function commonPrefix(before: string, after: string): number {
  if (kept && kept.after === after && kept.before === before) return kept.length;
  const limit = Math.min(before.length, after.length);
  let length = 0;
  // Native comparisons of shrinking chunks instead of a loop over every unit.
  for (let chunk = 4096; chunk >= 1; chunk >>= 3) {
    while (length + chunk <= limit && after.startsWith(before.slice(length, length + chunk), length)) {
      length += chunk;
    }
  }
  while (length < limit && before.charCodeAt(length) === after.charCodeAt(length)) length += 1;
  return length;
}
