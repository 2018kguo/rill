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
    while (
      length + chunk <= limit &&
      after.slice(length, length + chunk) === before.slice(length, length + chunk)
    ) {
      length += chunk;
    }
  }
  while (length < limit && before.charCodeAt(length) === after.charCodeAt(length)) length += 1;
  return length;
}

/**
 * A map that keeps its most recently used entries: at most `size` of them, and, when
 * `weigh` is given, at most `bytes` of what it estimates they hold. Keys that are texts
 * (a message, a block) can be long, and a page that streams without saying so offers a
 * new, longer one every update: the bytes bound keeps that linear.
 */
export class Lru<K, V> {
  readonly #entries = new Map<K, V>();
  readonly #weights = new Map<K, number>();
  readonly #size: number;
  readonly #bytes: number;
  readonly #weigh: ((key: K, value: V) => number) | undefined;
  #total = 0;

  constructor(size: number, bytes = Infinity, weigh?: (key: K, value: V) => number) {
    this.#size = size;
    this.#bytes = bytes;
    this.#weigh = weigh;
  }

  get(key: K): V | undefined {
    const value = this.#entries.get(key);
    if (value !== undefined) {
      this.#entries.delete(key);
      this.#entries.set(key, value);
    }
    return value;
  }

  set(key: K, value: V): void {
    this.#forget(key);
    const weight = this.#weigh?.(key, value) ?? 0;
    // One entry over the bound is not kept at all.
    if (weight > this.#bytes) return;
    this.#entries.set(key, value);
    if (this.#weigh) {
      this.#weights.set(key, weight);
      this.#total += weight;
    }
    while (this.#entries.size > this.#size || this.#total > this.#bytes) {
      this.#forget(this.#entries.keys().next().value as K);
    }
  }

  #forget(key: K): void {
    if (!this.#entries.delete(key)) return;
    this.#total -= this.#weights.get(key) ?? 0;
    this.#weights.delete(key);
  }
}
