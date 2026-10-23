The error comes from mutating a list while iterating over it. Here's what happens step by step:

1. `for item in items:` walks the list by **index**.
2. `items.remove(item)` shifts every later element one slot to the left.
3. The loop's index still advances, so the element that slid into the current slot is *skipped*.

### Fix

Build a new list instead of editing the old one:

```python
def drop_expired(items: list[dict], now: float) -> list[dict]:
    """Return the items whose deadline is still in the future."""
    return [item for item in items if item["deadline"] > now]


if __name__ == "__main__":
    import time

    queue = [{"id": 1, "deadline": time.time() + 60}, {"id": 2, "deadline": 0}]
    print(drop_expired(queue, time.time()))  # -> [{'id': 1, ...}]
```

If you must edit in place (say, other code holds a reference to the list), iterate over a copy:

```py
for item in items[:]:
    if item["deadline"] <= now:
        items.remove(item)
```

- [x] Reproduced the skip with two expired items in a row
- [x] Verified the comprehension keeps order
- [ ] Benchmark against `filter()` for 1M items

> **Note:** `list.remove` is O(n), so the in-place version is O(n²). For large queues prefer the comprehension or a `deque`.

See the [Python docs on mutable sequences](https://docs.python.org/3/library/stdtypes.html#mutable-sequence-types) for the full list of operations.
