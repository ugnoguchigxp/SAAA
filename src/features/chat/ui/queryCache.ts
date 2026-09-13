import type { UiData } from "./api";
export type QuerySnapshot = { data?: UiData; error?: string; loading: boolean };
const EMPTY: QuerySnapshot = { loading: false };
type Entry = {
  value: QuerySnapshot;
  subscribers: Map<symbol, () => Promise<UiData>>;
  listeners: Set<() => void>;
  timer?: ReturnType<typeof setTimeout>;
  pending?: Promise<void>;
  generation: number;
};
/** Queries are shared by conversation+source, never by an arbitrary URL. No subscribers = no work. */
export class UiQueryCache {
  private entries = new Map<string, Entry>();
  constructor(private interval = 15_000) {}
  snapshot(key: string): QuerySnapshot {
    return this.entries.get(key)?.value ?? EMPTY;
  }
  subscribe(key: string, fetch: () => Promise<UiData>, listener: () => void): () => void {
    let entry = this.entries.get(key);
    if (!entry) {
      entry = { value: EMPTY, subscribers: new Map(), listeners: new Set(), generation: 0 };
      this.entries.set(key, entry);
    }
    const token = Symbol();
    entry.subscribers.set(token, fetch);
    entry.listeners.add(listener);
    if (entry.subscribers.size === 1) void this.refresh(key);
    return () => {
      entry.subscribers.delete(token);
      entry.listeners.delete(listener);
      if (!entry.subscribers.size) {
        clearTimeout(entry.timer);
        entry.generation++;
        this.entries.delete(key); // a late result cannot restore a disposed entry
      }
    };
  }
  async refresh(key: string): Promise<void> {
    const entry = this.entries.get(key);
    if (!entry || !entry.subscribers.size) return;
    if (entry.pending) return entry.pending;
    clearTimeout(entry.timer);
    const generation = entry.generation;
    const fetch = entry.subscribers.values().next().value!;
    entry.value = { ...entry.value, loading: true };
    entry.pending = Promise.resolve().then(async () => {
      if (generation !== entry.generation || !entry.subscribers.size) return;
      try {
        const data = await fetch();
        if (generation === entry.generation) entry.value = { data, loading: false };
      } catch {
        if (generation === entry.generation)
          entry.value = { data: entry.value.data, error: "unavailable", loading: false };
      } finally {
        entry.pending = undefined;
        if (generation === entry.generation && entry.subscribers.size) {
          entry.listeners.forEach((fn) => fn());
          entry.timer = setTimeout(() => void this.refresh(key), this.interval);
        }
      }
    });
    entry.listeners.forEach((fn) => fn());
    return entry.pending;
  }
  get size() {
    return this.entries.size;
  }
}
export const uiQueries = new UiQueryCache();
