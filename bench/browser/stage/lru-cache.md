# An LRU cache in four languages

An LRU (least recently used) cache keeps a fixed number of entries and, when it is full, evicts the entry that was used longest ago. Both `get` and `put` should run in **O(1)** time. The standard design pairs two structures:

1. A **hash map** from key to entry, for O(1) lookup.
2. A **doubly linked list** ordered by recency, for O(1) moves and evictions.

On every `get` hit, move the entry to the front. On every `put`, insert at the front, and if the cache is over capacity, drop the entry at the back.

| Operation | Map | List | Time |
|---|---|---|---|
| `get` hit | lookup | move to front | O(1) |
| `get` miss | lookup | — | O(1) |
| `put` new key | insert | push front, maybe pop back | O(1) |
| `put` existing key | lookup | update + move to front | O(1) |

## TypeScript

JavaScript's `Map` already remembers insertion order, so deleting and re-inserting a key moves it to the "newest" end. That gives a compact implementation without writing a linked list by hand:

```ts
export class LruCache<K, V> {
  readonly #capacity: number;
  readonly #entries = new Map<K, V>();

  constructor(capacity: number) {
    if (!Number.isInteger(capacity) || capacity < 1) {
      throw new RangeError(`capacity must be a positive integer, got ${capacity}`);
    }
    this.#capacity = capacity;
  }

  get size(): number {
    return this.#entries.size;
  }

  get(key: K): V | undefined {
    if (!this.#entries.has(key)) return undefined;
    const value = this.#entries.get(key)!;
    // Re-insert to mark as most recently used.
    this.#entries.delete(key);
    this.#entries.set(key, value);
    return value;
  }

  put(key: K, value: V): void {
    if (this.#entries.has(key)) this.#entries.delete(key);
    this.#entries.set(key, value);
    if (this.#entries.size > this.#capacity) {
      // The first key in iteration order is the least recently used.
      const oldest = this.#entries.keys().next().value as K;
      this.#entries.delete(oldest);
    }
  }
}
```

```ts
import { describe, expect, it } from "vitest";
import { LruCache } from "./lru-cache";

describe("LruCache", () => {
  it("evicts the least recently used entry", () => {
    const cache = new LruCache<string, number>(2);
    cache.put("a", 1);
    cache.put("b", 2);
    expect(cache.get("a")).toBe(1); // "a" is now the most recent
    cache.put("c", 3); // evicts "b"
    expect(cache.get("b")).toBeUndefined();
    expect(cache.get("a")).toBe(1);
    expect(cache.get("c")).toBe(3);
  });

  it("updates an existing key without growing", () => {
    const cache = new LruCache<string, number>(2);
    cache.put("a", 1);
    cache.put("a", 10);
    expect(cache.size).toBe(1);
    expect(cache.get("a")).toBe(10);
  });

  it("rejects a non-positive capacity", () => {
    expect(() => new LruCache(0)).toThrow(RangeError);
  });
});
```

## Python

`collections.OrderedDict` has `move_to_end` and `popitem(last=False)`, which are exactly the two operations an LRU cache needs:

```python
from collections import OrderedDict
from typing import Generic, Hashable, Optional, TypeVar

K = TypeVar("K", bound=Hashable)
V = TypeVar("V")


class LruCache(Generic[K, V]):
    def __init__(self, capacity: int) -> None:
        if capacity < 1:
            raise ValueError(f"capacity must be positive, got {capacity}")
        self._capacity = capacity
        self._entries: "OrderedDict[K, V]" = OrderedDict()

    def __len__(self) -> int:
        return len(self._entries)

    def get(self, key: K) -> Optional[V]:
        if key not in self._entries:
            return None
        self._entries.move_to_end(key)
        return self._entries[key]

    def put(self, key: K, value: V) -> None:
        if key in self._entries:
            self._entries.move_to_end(key)
        self._entries[key] = value
        if len(self._entries) > self._capacity:
            self._entries.popitem(last=False)
```

```python
import pytest

from lru_cache import LruCache


def test_evicts_least_recently_used():
    cache = LruCache[str, int](2)
    cache.put("a", 1)
    cache.put("b", 2)
    assert cache.get("a") == 1
    cache.put("c", 3)
    assert cache.get("b") is None
    assert cache.get("a") == 1
    assert cache.get("c") == 3


def test_update_keeps_size():
    cache = LruCache[str, int](2)
    cache.put("a", 1)
    cache.put("a", 10)
    assert len(cache) == 1
    assert cache.get("a") == 10


def test_rejects_zero_capacity():
    with pytest.raises(ValueError):
        LruCache(0)
```

## Rust

Rust's standard library has no ordered hash map, so this version keeps entries in a `Vec` used as a slab and links them by index. Indices instead of pointers keep it entirely safe:

```rust
use std::collections::HashMap;
use std::hash::Hash;

struct Entry<K, V> {
    key: K,
    value: V,
    prev: Option<usize>,
    next: Option<usize>,
}

pub struct LruCache<K, V> {
    capacity: usize,
    map: HashMap<K, usize>,
    entries: Vec<Entry<K, V>>,
    head: Option<usize>, // most recently used
    tail: Option<usize>, // least recently used
}

impl<K: Hash + Eq + Clone, V> LruCache<K, V> {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be positive");
        Self { capacity, map: HashMap::new(), entries: Vec::with_capacity(capacity), head: None, tail: None }
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        let index = *self.map.get(key)?;
        self.detach(index);
        self.push_front(index);
        Some(&self.entries[index].value)
    }

    pub fn put(&mut self, key: K, value: V) {
        if let Some(&index) = self.map.get(&key) {
            self.entries[index].value = value;
            self.detach(index);
            self.push_front(index);
            return;
        }
        let index = if self.entries.len() < self.capacity {
            self.entries.push(Entry { key: key.clone(), value, prev: None, next: None });
            self.entries.len() - 1
        } else {
            // Reuse the least recently used slot.
            let index = self.tail.expect("a full cache has a tail");
            self.detach(index);
            let old = std::mem::replace(&mut self.entries[index], Entry { key: key.clone(), value, prev: None, next: None });
            self.map.remove(&old.key);
            index
        };
        self.map.insert(key, index);
        self.push_front(index);
    }

    fn detach(&mut self, index: usize) {
        let (prev, next) = (self.entries[index].prev, self.entries[index].next);
        match prev {
            Some(p) => self.entries[p].next = next,
            None => self.head = next,
        }
        match next {
            Some(n) => self.entries[n].prev = prev,
            None => self.tail = prev,
        }
    }

    fn push_front(&mut self, index: usize) {
        self.entries[index].prev = None;
        self.entries[index].next = self.head;
        if let Some(h) = self.head {
            self.entries[h].prev = Some(index);
        }
        self.head = Some(index);
        if self.tail.is_none() {
            self.tail = Some(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LruCache;

    #[test]
    fn evicts_least_recently_used() {
        let mut cache = LruCache::new(2);
        cache.put("a", 1);
        cache.put("b", 2);
        assert_eq!(cache.get(&"a"), Some(&1));
        cache.put("c", 3);
        assert_eq!(cache.get(&"b"), None);
        assert_eq!(cache.get(&"c"), Some(&3));
    }
}
```

## Go

Go's `container/list` is a doubly linked list, so the map can point straight at list elements:

```go
package lru

import "container/list"

type entry[K comparable, V any] struct {
	key   K
	value V
}

type Cache[K comparable, V any] struct {
	capacity int
	order    *list.List // front = most recently used
	items    map[K]*list.Element
}

func New[K comparable, V any](capacity int) *Cache[K, V] {
	if capacity < 1 {
		panic("lru: capacity must be positive")
	}
	return &Cache[K, V]{capacity: capacity, order: list.New(), items: make(map[K]*list.Element)}
}

func (c *Cache[K, V]) Get(key K) (V, bool) {
	if element, ok := c.items[key]; ok {
		c.order.MoveToFront(element)
		return element.Value.(*entry[K, V]).value, true
	}
	var zero V
	return zero, false
}

func (c *Cache[K, V]) Put(key K, value V) {
	if element, ok := c.items[key]; ok {
		element.Value.(*entry[K, V]).value = value
		c.order.MoveToFront(element)
		return
	}
	c.items[key] = c.order.PushFront(&entry[K, V]{key, value})
	if c.order.Len() > c.capacity {
		oldest := c.order.Back()
		c.order.Remove(oldest)
		delete(c.items, oldest.Value.(*entry[K, V]).key)
	}
}
```

```go
package lru

import "testing"

func TestEvictsLeastRecentlyUsed(t *testing.T) {
	cache := New[string, int](2)
	cache.Put("a", 1)
	cache.Put("b", 2)
	if v, _ := cache.Get("a"); v != 1 {
		t.Fatalf("get a = %d, want 1", v)
	}
	cache.Put("c", 3)
	if _, ok := cache.Get("b"); ok {
		t.Fatal("b should have been evicted")
	}
}
```

## Which one to reach for

- **Production code:** use the ecosystem's battle-tested package (`lru-cache` on npm, `functools.lru_cache` for memoizing Python functions, the `lru` crate, `hashicorp/golang-lru`). They add TTLs, size-based eviction and thread safety.
- **Interviews and learning:** the Rust and Go versions show the real data structure; the TypeScript and Python versions show how far a language's ordered map gets you.
- **Concurrency:** none of these are thread-safe. Wrap them in a mutex, or shard the cache by key hash to reduce contention.
