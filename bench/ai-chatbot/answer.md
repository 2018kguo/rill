# Building a rate-limited job queue

Here is a complete design for a small job queue with retries, rate limits and a dashboard. It covers the data model, the worker loop, the HTTP API, and how to operate it in production. Each section stands on its own, so skip to the part you need.

## Requirements

1. Jobs are durable: a crash of any process loses no accepted job.
2. Each job runs **at least once**; handlers must be idempotent.
3. Workers scale horizontally without coordination beyond the database.
4. A per-tenant rate limit caps how many jobs run each second.
5. Failed jobs retry with exponential backoff and jitter, up to a limit.
6. Jobs that exhaust their retries move to a *dead-letter* table.
7. Operators can pause a queue, drain it, and replay dead jobs.
8. Every state change is observable as a metric and a log line.
9. Scheduling latency stays under 50 ms at the median.
10. The design runs on one Postgres instance up to ~2,000 jobs per second.
11. Priorities: `high`, `normal` and `low`, with no starvation of `low`.
12. Jobs can be scheduled for a time in the future (`run_at`).
13. A job may declare a timeout; the worker cancels it when exceeded.
14. Handlers receive a context with the attempt number and a logger.
15. Payloads are JSON, at most 256 KB; larger data goes to object storage.
16. Tenants are isolated: one tenant's backlog never delays another's.
17. Deploys are rolling; old and new workers coexist safely.
18. Shutdown is graceful: in-flight jobs finish or are released.
19. Unique jobs: an optional key deduplicates enqueues within a window.
20. Batches: a parent job completes when all of its children do.
21. Cron schedules enqueue jobs on a calendar, exactly once per tick.
22. Admin actions are audited with the actor and the reason.
23. Retention: finished jobs are pruned after 7 days.
24. The API is versioned and backwards compatible for a year.
25. Local development needs only `docker compose up`.
26. Tests run against a real database, not mocks.
27. Load tests are part of CI for every change to the worker loop.
28. Metrics export to Prometheus; traces export over OTLP.
29. Secrets come from the environment, never from the database.
30. The whole system is one binary plus Postgres.

## Data model

| Table | Purpose | Key columns | Notes |
|:--|:--|:--|:--|
| `jobs` | Every accepted job | `id`, `queue`, `tenant`, `state`, `run_at` | Partial index on runnable jobs |
| `attempts` | One row per execution | `job_id`, `attempt`, `started_at`, `error` | Append-only |
| `dead_jobs` | Jobs out of retries | `job_id`, `failed_at`, `last_error` | Replayable |
| `queues` | Per-queue settings | `name`, `paused`, `rate_per_sec` | Cached by workers for 5 s |
| `tenants` | Per-tenant limits | `id`, `rate_per_sec`, `burst` | Token bucket parameters |
| `schedules` | Cron definitions | `name`, `cron`, `next_tick` | Advanced in the same transaction |
| `audit_log` | Admin actions | `actor`, `action`, `reason`, `at` | Never pruned |

```sql
create table jobs (
  id          bigint generated always as identity primary key,
  queue       text        not null,
  tenant      text        not null,
  priority    smallint    not null default 1,
  state       text        not null default 'ready'
              check (state in ('ready', 'running', 'done', 'failed', 'dead')),
  payload     jsonb       not null,
  attempt     int         not null default 0,
  max_attempts int        not null default 8,
  run_at      timestamptz not null default now(),
  locked_by   text,
  locked_until timestamptz,
  unique_key  text,
  created_at  timestamptz not null default now()
);

create index jobs_runnable on jobs (queue, priority desc, run_at)
  where state = 'ready';
create unique index jobs_unique on jobs (queue, unique_key)
  where unique_key is not null and state in ('ready', 'running');
```

## The worker loop

A worker claims a batch with `FOR UPDATE SKIP LOCKED`, so many workers can poll the same queue without blocking each other. Claiming sets a lease (`locked_until`); a reaper returns jobs whose lease expired to `ready`.

```ts
type Job = { id: number; queue: string; tenant: string; payload: unknown; attempt: number };

export async function claim(db: Db, queue: string, worker: string, limit = 32): Promise<Job[]> {
  const { rows } = await db.query<Job>(
    `update jobs set state = 'running', attempt = attempt + 1,
            locked_by = $2, locked_until = now() + interval '30 seconds'
      where id in (
        select id from jobs
         where queue = $1 and state = 'ready' and run_at <= now()
         order by priority desc, run_at
         limit $3
         for update skip locked)
     returning id, queue, tenant, payload, attempt`,
    [queue, worker, limit],
  );
  return rows;
}

export async function runLoop(db: Db, queue: string, handlers: Record<string, Handler>, signal: AbortSignal) {
  const worker = `${hostname()}:${process.pid}`;
  while (!signal.aborted) {
    const jobs = await claim(db, queue, worker);
    if (jobs.length === 0) {
      await sleep(jitter(250), signal);
      continue;
    }
    await Promise.all(jobs.map((job) => execute(db, job, handlers, signal)));
  }
}
```

Backoff doubles per attempt with full jitter, capped at ten minutes:

```python
import random

def backoff(attempt: int, base: float = 1.0, cap: float = 600.0) -> float:
    """Seconds to wait before the next attempt (full jitter)."""
    exp = min(cap, base * 2 ** attempt)
    return random.uniform(0, exp)

assert all(0 <= backoff(n) <= 600 for n in range(20))
```

## Rate limiting

Each tenant has a token bucket kept in memory per worker and reconciled through the database every second. A worker only claims jobs for tenants with tokens left.

```rust
use std::time::Instant;

pub struct Bucket {
    capacity: f64,
    tokens: f64,
    rate_per_sec: f64,
    last: Instant,
}

impl Bucket {
    pub fn new(capacity: f64, rate_per_sec: f64) -> Self {
        Self { capacity, tokens: capacity, rate_per_sec, last: Instant::now() }
    }

    /// Takes one token if available.
    pub fn try_take(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate_per_sec).min(self.capacity);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}
```

| Tenant tier | Rate (jobs/s) | Burst | Max running | Dead-letter alert |
|:--|--:|--:|--:|:--|
| Free | 5 | 10 | 4 | daily digest |
| Team | 50 | 100 | 32 | after 10 jobs |
| Business | 250 | 500 | 128 | after 1 job |
| Enterprise | 1,000 | 2,000 | 512 | paged at once |

## HTTP API

| Method | Path | Body | Returns |
|:--|:--|:--|:--|
| `POST` | `/v1/jobs` | `{ queue, payload, run_at?, unique_key? }` | `201` with the job |
| `GET` | `/v1/jobs/:id` | | the job and its attempts |
| `POST` | `/v1/jobs/:id/cancel` | `{ reason }` | `202` |
| `POST` | `/v1/queues/:name/pause` | `{ reason }` | `204` |
| `POST` | `/v1/queues/:name/resume` | | `204` |
| `POST` | `/v1/dead/:id/replay` | | `201` with the new job |

```bash
curl -sS -X POST https://jobs.example.com/v1/jobs \
  -H "authorization: Bearer $TOKEN" \
  -H "content-type: application/json" \
  -d '{"queue":"emails","payload":{"to":"ada@example.com","template":"welcome"}}'
```

## Operating it

- **Dashboards**: queue depth per priority, claim latency, attempts per second, dead jobs per hour.
- **Alerts**: depth growing for 10 minutes, any `high` job waiting over 30 s, the reaper returning more than 1% of claims.
- **Runbook**: pause the queue, inspect `attempts.error` for the top failures, fix forward, then replay from `dead_jobs` in batches of 500.
- **Capacity**: one worker process handles about 400 short jobs per second; add workers before adding database size.
- **Upgrades**: migrations are additive; a column is dropped only two releases after the code stops reading it.

That is the whole system: one table of jobs, a claim query, a bucket per tenant, and a small API. End of answer.
