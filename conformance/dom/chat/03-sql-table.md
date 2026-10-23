## Query plan comparison

I ran both versions against a copy of production (42M rows in `orders`, 1.3M in `customers`):

| Variant | Plan | Rows scanned | Time (p50) | Time (p99) | Notes |
|:--|:--|--:|--:|--:|:-:|
| Original | Seq Scan on `orders` | 42,118,904 | 8.4 s | 12.9 s | ❌ |
| With index on `(customer_id, created_at)` | Index Scan | 3,212 | 4 ms | 19 ms | ✅ |
| Partial index `WHERE status = 'open'` | Index Only Scan | 211 | 0.9 ms | 3 ms | ✅<br>needs `VACUUM` |
| Materialized view | Seq Scan on MV | 18,002 | 31 ms | 55 ms | stale up to 5 min |

The composite index is the safest change. Create it without locking writes:

```sql
CREATE INDEX CONCURRENTLY IF NOT EXISTS orders_customer_created_idx
  ON orders (customer_id, created_at DESC);

ANALYZE orders;
```

Then rewrite the query so the planner can use the index for both the filter and the sort:

```sql
SELECT o.id, o.total_cents, o.created_at
FROM orders AS o
WHERE o.customer_id = $1
  AND o.created_at >= now() - interval '90 days'
ORDER BY o.created_at DESC
LIMIT 50;
```

**Caveats**

* `CREATE INDEX CONCURRENTLY` can't run inside a transaction block.
* On a busy table it may take a while; watch `pg_stat_progress_create_index`.
* If the build fails it leaves an `INVALID` index behind — drop it and retry.

---

*Measured with `EXPLAIN (ANALYZE, BUFFERS)`; numbers are the median of five runs.*
