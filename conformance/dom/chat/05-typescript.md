Here's a typed event emitter that infers the payload from the event name:

```typescript
type Listener<T> = (payload: T) => void;

export class Emitter<Events extends Record<string, unknown>> {
  private listeners = new Map<keyof Events, Set<Listener<never>>>();

  on<K extends keyof Events>(event: K, listener: Listener<Events[K]>): () => void {
    let set = this.listeners.get(event);
    if (!set) {
      set = new Set();
      this.listeners.set(event, set);
    }
    set.add(listener as Listener<never>);
    return () => set.delete(listener as Listener<never>);
  }

  emit<K extends keyof Events>(event: K, payload: Events[K]): void {
    for (const listener of this.listeners.get(event) ?? []) {
      (listener as Listener<Events[K]>)(payload);
    }
  }
}

// Usage
const bus = new Emitter<{ login: { userId: string }; logout: undefined }>();
const off = bus.on("login", ({ userId }) => console.log(`hello ${userId}`));
bus.emit("login", { userId: "u_123" });
off();
```

A few design notes:

1. **`on` returns an unsubscribe function.** It's easier to use correctly in React effects than a separate `off(event, listener)`:

   ```tsx
   useEffect(() => bus.on("logout", () => navigate("/login")), []);
   ```

2. **Listeners are stored in a `Set`**, so registering the same function twice is a no-op.
3. `emit` iterates a live set; a listener that unsubscribes *during* emit is fine because `Set` iteration tolerates deletion.

Install nothing — this is ~30 lines. If you need wildcard events or async listeners, look at [mitt](https://github.com/developit/mitt) or [eventemitter3](https://www.npmjs.com/package/eventemitter3).

Press <kbd>Ctrl</kbd>+<kbd>C</kbd> to stop the dev server.
