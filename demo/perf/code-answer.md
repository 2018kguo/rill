Here's a complete, production-ready job queue backed by Postgres. It uses `SELECT ... FOR UPDATE SKIP LOCKED` so many workers can poll the same table without stepping on each other, retries with exponential backoff, and a visibility timeout so crashed workers don't lose jobs.

```typescript
import { setTimeout as sleep } from "node:timers/promises";
import type { Pool, PoolClient } from "pg";

export type JobStatus = "queued" | "running" | "succeeded" | "failed" | "dead";

export interface Job<Payload = unknown> {
  id: string;
  queue: string;
  payload: Payload;
  status: JobStatus;
  attempts: number;
  maxAttempts: number;
  runAt: Date;
  lockedUntil: Date | null;
  lastError: string | null;
  createdAt: Date;
  updatedAt: Date;
}

export interface EnqueueOptions {
  /** Delay before the job becomes visible to workers, in milliseconds. */
  delayMs?: number;
  /** How many times to try before the job is marked dead. */
  maxAttempts?: number;
  /** Jobs with the same key are deduplicated while queued. */
  dedupeKey?: string;
}

export interface WorkerOptions {
  queue: string;
  /** How many jobs this worker runs at once. */
  concurrency?: number;
  /** How long a claimed job stays invisible to other workers. */
  visibilityTimeoutMs?: number;
  /** How long to wait between polls when the queue is empty. */
  idleDelayMs?: number;
  signal?: AbortSignal;
}

export type Handler<Payload> = (job: Job<Payload>, signal: AbortSignal) => Promise<void>;

const SCHEMA = `
  CREATE TABLE IF NOT EXISTS jobs (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    queue         text        NOT NULL,
    payload       jsonb       NOT NULL,
    status        text        NOT NULL DEFAULT 'queued',
    attempts      int         NOT NULL DEFAULT 0,
    max_attempts  int         NOT NULL DEFAULT 5,
    run_at        timestamptz NOT NULL DEFAULT now(),
    locked_until  timestamptz,
    last_error    text,
    dedupe_key    text,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
  );
  CREATE INDEX IF NOT EXISTS jobs_ready_idx
    ON jobs (queue, run_at) WHERE status IN ('queued', 'running');
  CREATE UNIQUE INDEX IF NOT EXISTS jobs_dedupe_idx
    ON jobs (queue, dedupe_key) WHERE status = 'queued' AND dedupe_key IS NOT NULL;
`;

export class JobQueue {
  constructor(private readonly pool: Pool) {}

  async migrate(): Promise<void> {
    await this.pool.query(SCHEMA);
  }

  async enqueue<Payload>(
    queue: string,
    payload: Payload,
    options: EnqueueOptions = {}
  ): Promise<string | null> {
    const { delayMs = 0, maxAttempts = 5, dedupeKey = null } = options;
    const result = await this.pool.query<{ id: string }>(
      `INSERT INTO jobs (queue, payload, max_attempts, run_at, dedupe_key)
       VALUES ($1, $2, $3, now() + ($4 || ' milliseconds')::interval, $5)
       ON CONFLICT (queue, dedupe_key) WHERE status = 'queued' AND dedupe_key IS NOT NULL
       DO NOTHING
       RETURNING id`,
      [queue, JSON.stringify(payload), maxAttempts, String(delayMs), dedupeKey]
    );
    return result.rows[0]?.id ?? null;
  }

  /** Claims up to `limit` ready jobs, invisible to others until the timeout passes. */
  private async claim(
    client: PoolClient,
    queue: string,
    limit: number,
    visibilityTimeoutMs: number
  ): Promise<Job[]> {
    const result = await client.query(
      `UPDATE jobs SET
         status = 'running',
         attempts = attempts + 1,
         locked_until = now() + ($3 || ' milliseconds')::interval,
         updated_at = now()
       WHERE id IN (
         SELECT id FROM jobs
         WHERE queue = $1
           AND run_at <= now()
           AND (status = 'queued' OR (status = 'running' AND locked_until < now()))
         ORDER BY run_at
         LIMIT $2
         FOR UPDATE SKIP LOCKED
       )
       RETURNING *`,
      [queue, limit, String(visibilityTimeoutMs)]
    );
    return result.rows.map(toJob);
  }

  private async complete(id: string): Promise<void> {
    await this.pool.query(
      `UPDATE jobs SET status = 'succeeded', locked_until = NULL, updated_at = now()
       WHERE id = $1`,
      [id]
    );
  }

  private async fail(job: Job, error: unknown): Promise<void> {
    const message = error instanceof Error ? error.stack ?? error.message : String(error);
    const dead = job.attempts >= job.maxAttempts;
    // 2^attempts seconds with jitter, capped at 10 minutes.
    const backoffMs = Math.min(600_000, 1000 * 2 ** job.attempts) * (0.5 + Math.random());
    await this.pool.query(
      `UPDATE jobs SET
         status = $2,
         last_error = $3,
         locked_until = NULL,
         run_at = now() + ($4 || ' milliseconds')::interval,
         updated_at = now()
       WHERE id = $1`,
      [job.id, dead ? "dead" : "queued", message.slice(0, 10_000), String(Math.round(backoffMs))]
    );
  }

  async work<Payload>(handler: Handler<Payload>, options: WorkerOptions): Promise<void> {
    const {
      queue,
      concurrency = 4,
      visibilityTimeoutMs = 30_000,
      idleDelayMs = 1000,
      signal,
    } = options;
    const running = new Set<Promise<void>>();

    while (!signal?.aborted) {
      const free = concurrency - running.size;
      if (free === 0) {
        await Promise.race(running);
        continue;
      }
      const client = await this.pool.connect();
      let jobs: Job[];
      try {
        jobs = await this.claim(client, queue, free, visibilityTimeoutMs);
      } finally {
        client.release();
      }
      if (jobs.length === 0) {
        await sleep(idleDelayMs, undefined, { signal }).catch(() => undefined);
        continue;
      }
      for (const job of jobs) {
        const controller = new AbortController();
        const timer = setTimeout(() => controller.abort(), visibilityTimeoutMs);
        const task = handler(job as Job<Payload>, controller.signal)
          .then(() => this.complete(job.id))
          .catch((error: unknown) => this.fail(job, error))
          .finally(() => {
            clearTimeout(timer);
            running.delete(task);
          });
        running.add(task);
      }
    }
    await Promise.allSettled(running);
  }
}

function toJob(row: Record<string, unknown>): Job {
  return {
    id: row.id as string,
    queue: row.queue as string,
    payload: row.payload,
    status: row.status as JobStatus,
    attempts: row.attempts as number,
    maxAttempts: row.max_attempts as number,
    runAt: row.run_at as Date,
    lockedUntil: (row.locked_until as Date | null) ?? null,
    lastError: (row.last_error as string | null) ?? null,
    createdAt: row.created_at as Date,
    updatedAt: row.updated_at as Date,
  };
}
```

Usage:

```typescript
const queue = new JobQueue(pool);
await queue.migrate();
await queue.enqueue("emails", { to: "ada@example.com", template: "welcome" }, { dedupeKey: "welcome:ada" });

const controller = new AbortController();
process.on("SIGTERM", () => controller.abort());
await queue.work<{ to: string; template: string }>(
  async (job) => sendEmail(job.payload.to, job.payload.template),
  { queue: "emails", concurrency: 8, signal: controller.signal }
);
```

**Why `SKIP LOCKED`:** without it, every worker would block on the same first row; with it, each worker takes the next unlocked rows and moves on, so throughput scales with workers until the table's index becomes the bottleneck.
