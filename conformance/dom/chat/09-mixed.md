Great question! There are **three** ways to deploy this, depending on your constraints.

### Option A — Docker Compose (simplest)

```yaml
services:
  api:
    image: ghcr.io/acme/api:1.4.2
    ports: ["8080:8080"]
    environment:
      DATABASE_URL: postgres://app:secret@db:5432/app
    depends_on: [db]
  db:
    image: postgres:16
    volumes: [pgdata:/var/lib/postgresql/data]
volumes:
  pgdata: {}
```

### Option B — Kubernetes

Use a `Deployment` plus a `Service`:

```bash
kubectl create namespace acme
kubectl -n acme apply -f k8s/
kubectl -n acme rollout status deploy/api --timeout=120s
```

### Option C — Serverless

Works if requests finish in under ~30 s. Cold starts add **200–800 ms**.

---

| | Compose | Kubernetes | Serverless |
|---|---|---|---|
| Setup time | 10 min | 1–2 days | 30 min |
| Scaling | Manual | Automatic (HPA) | Automatic |
| Cost at idle | VM cost | Cluster cost | ~$0 |
| Best for | Small teams | Many services | Spiky traffic |

My recommendation: **start with Compose**, move to Kubernetes when you have more than ~5 services. ~~Serverless~~ is great for webhooks but awkward for long-lived connections.

```diff
- const timeout = 5000;
+ const timeout = Number(process.env.TIMEOUT_MS ?? 5000);
```

```
plain fenced block without a language
    with indentation preserved
```

```js startLine=10
const answer = 42;
console.log(answer);
```

```rust noLineNumbers
fn main() {
    println!("hello");
}
```
