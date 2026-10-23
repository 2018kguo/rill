# Choosing a rate limiter for a public API

Short answer: use a **token bucket** per API key at the edge, back it with a **sliding-window log** for the few endpoints where bursts are expensive, and return `429` with a `Retry-After` header. The rest of this answer walks through why, compares the common algorithms, and ends with a production-ready implementation.

## What you are actually protecting

Rate limits exist to protect three different things, and the right algorithm depends on which one matters most:

1. **Capacity** — keeping a shared database or model server under its saturation point.
2. **Fairness** — stopping one tenant from starving the others.
3. **Cost** — capping spend on metered upstreams (LLM tokens, SMS, card networks).

> If you only remember one thing: limit on the resource that actually runs out, not on requests. A request that streams 4,000 tokens is not the same as one that returns a cached `200`.

## The algorithms, compared

| Algorithm | Burst behavior | Memory per key | Precision | Best for |
|---|---|---|---|---|
| Fixed window | Up to 2× limit at window edges<br>(boundary burst) | 1 counter | Low | Internal tools, rough quotas |
| Sliding window log | Exact | 1 timestamp per request<br>(can be large) | Exact | Expensive endpoints, billing |
| Sliding window counter | Smoothed estimate | 2 counters | ~99% | General API limits |
| Token bucket | Allows bursts up to bucket size | 2 numbers<br>(tokens, last refill) | Exact for rate | Public APIs, SDK-friendly |
| Leaky bucket | No bursts; constant drain | Queue or counter | Exact | Traffic shaping, upstream calls |
| Concurrency limit | N in flight, no time window | 1 counter | Exact | Streaming, long requests |
| GCRA | Same as token bucket | 1 timestamp | Exact | Redis, high-throughput edges |
| Adaptive (AIMD) | Backs off under errors | Few counters | Feedback-driven | Calling flaky upstreams |

A few notes on the table: the sliding window counter is what most API gateways ship because it is cheap and *good enough*; GCRA<sup>1</sup> is a token bucket written as a single timestamp, which makes it atomic in one Redis command; and concurrency limits are the only sane option for long-lived streams, because a time window says nothing about a request that lasts ninety seconds.

## How a token bucket works

A bucket holds up to $$b$$ tokens and refills at $$r$$ tokens per second. A request costing $$c$$ tokens is allowed when the bucket has at least $$c$$:

$$
\text{tokens}(t) = \min\left(b,\ \text{tokens}(t_0) + r \cdot (t - t_0)\right)
$$

The longest burst a client can send at once is $$b$$, and over any interval of length $$T$$ it can send at most $$b + rT$$. Choosing $$b = r$$ gives "one second of burst", which is a sensible default for interactive clients.

### Weighing requests by cost

Charge each request what it costs you. For an LLM endpoint, a reasonable cost function is

$$
c = 1 + \left\lceil \frac{\text{prompt tokens} + 4 \cdot \text{completion tokens}}{1000} \right\rceil
$$

so a tiny request costs about one token and a long generation costs proportionally more. Estimate completion tokens from `max_tokens` up front, then **refund** the difference when the stream ends.

## Implementation

Here is a GCRA limiter as a single atomic Redis script. It needs one key per client and no background jobs:

```lua
-- KEYS[1]: limiter key, ARGV: now_ms, emission_interval_ms, burst_tolerance_ms, cost
local tat = tonumber(redis.call("GET", KEYS[1]) or ARGV[1])
local now = tonumber(ARGV[1])
local interval = tonumber(ARGV[2]) * tonumber(ARGV[4])
local new_tat = math.max(tat, now) + interval
local allow_at = new_tat - tonumber(ARGV[3])
if allow_at > now then
  return { 0, allow_at - now } -- denied, retry after (ms)
end
redis.call("SET", KEYS[1], new_tat, "PX", math.ceil(new_tat - now))
return { 1, 0 }
```

And the TypeScript side that calls it, with headers clients can act on:

```ts
import { createClient } from "redis";

const redis = createClient({ url: process.env.REDIS_URL });
const script = await redis.scriptLoad(GCRA_LUA);

export async function limit(key: string, cost = 1, perSecond = 10, burst = 20) {
  const interval = 1000 / perSecond;
  const [allowed, retryMs] = (await redis.evalSha(script, {
    keys: [`rl:${key}`],
    arguments: [String(Date.now()), String(interval), String(interval * burst), String(cost)],
  })) as [number, number];
  return {
    allowed: allowed === 1,
    headers: {
      "RateLimit-Limit": String(perSecond),
      "Retry-After": String(Math.ceil(retryMs / 1000)),
    },
  };
}
```

For a Python service the same idea fits in a decorator:

```python
import functools, time

def rate_limited(bucket_size: float, refill_per_sec: float):
    tokens, last = bucket_size, time.monotonic()

    def decorator(fn):
        @functools.wraps(fn)
        def wrapper(*args, **kwargs):
            nonlocal tokens, last
            now = time.monotonic()
            tokens = min(bucket_size, tokens + (now - last) * refill_per_sec)
            last = now
            if tokens < 1:
                raise RuntimeError(f"rate limited; retry in {(1 - tokens) / refill_per_sec:.2f}s")
            tokens -= 1
            return fn(*args, **kwargs)
        return wrapper
    return decorator
```

## Responses clients can rely on

| Situation | Status | Headers | Body |
|---|---|---|---|
| Allowed | `200` | `RateLimit-Remaining`, `RateLimit-Reset` | — |
| Over the per-key limit | `429` | `Retry-After: <seconds>` | `{"error":"rate_limited"}` |
| Over a concurrency limit | `429` | `Retry-After: 1` | `{"error":"too_many_streams"}` |
| Global overload | `503` | `Retry-After: <seconds>` | `{"error":"overloaded"}` |
| Quota exhausted for the month | `402` or `429` | `RateLimit-Policy` | link to billing |

Use `Retry-After` in **seconds**, keep the error codes stable, and document them; SDKs will build retry loops on top of whatever you send.

## Rollout checklist

- [x] Pick the limited resource (requests, tokens, or concurrent streams)
- [x] Start in **shadow mode**: compute decisions, log them, never block
- [ ] Compare shadow decisions with real traffic for a week
- [ ] Set limits at the p99.9 of legitimate per-key usage, plus headroom
- [ ] Enforce for new keys first, then everyone
- [ ] Alert on the `429` rate per key, not just the total

## Common mistakes

1. **Limiting by IP** behind a load balancer or NAT, which punishes whole offices. Limit by API key, and by IP only for unauthenticated routes.
2. **Counting retries as new requests** without jitter, which turns a brief overload into a synchronized retry storm. Clients should back off exponentially with full jitter: wait a random time in $$[0, \min(\text{cap}, \text{base} \cdot 2^n)]$$.
3. **Forgetting streams**: a time-window limit does nothing about a client holding 500 open connections. Add a concurrency limit.
4. **Hiding limits**: if clients cannot see `RateLimit-*` headers, they will find your limits by hitting them.

For the formal model, see the [Generic Cell Rate Algorithm](https://en.wikipedia.org/wiki/Generic_cell_rate_algorithm) and the IETF draft on [RateLimit header fields](https://datatracker.ietf.org/doc/draft-ietf-httpapi-ratelimit-headers/). Both are short and worth reading before you pick numbers.

<sup>1</sup> GCRA was designed for ATM networks; it is a token bucket expressed as a "theoretical arrival time", which is why it needs only one stored value per key.
