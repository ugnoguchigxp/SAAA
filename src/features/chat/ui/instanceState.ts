import { uiApi, type UiInstance } from './api';
type State = UiInstance['state'];
type Entry = { owners: number; value: State; base: State; version: number; dirty: boolean; pending?: Promise<void>; listeners: Set<() => void>; error: boolean };
/** Component-independent state. Writes are serialized so unmount cannot drop the last edit. */
export class InstanceStateStore {
  private entries = new Map<string, Entry>();
  constructor(private write = uiApi.state, private read = uiApi.load) {}
  initialize(instance: UiInstance) {
    if (!this.entries.has(instance.id)) this.entries.set(instance.id, { owners: 0, value: instance.state, base: instance.state, version: instance.stateVersion, dirty: false, listeners: new Set(), error: false });
    const entry = this.get(instance.id);
    if (!entry.dirty && !entry.pending && instance.stateVersion > entry.version) {
      entry.value = instance.state; entry.base = instance.state; entry.version = instance.stateVersion; entry.error = false;
      entry.listeners.forEach(fn => fn());
    }
    this.prune(instance.id);
  }
  retain(instance: UiInstance) {
    this.initialize(instance); const entry = this.get(instance.id); entry.owners++;
    return () => { entry.owners--; this.prune(); };
  }
  get(id: string) { return this.entries.get(id)!; }
  subscribe(id: string, listener: () => void) { const entry = this.get(id); entry.listeners.add(listener); return () => { entry.listeners.delete(listener); this.prune(); }; }
  update(id: string, key: string, value: State[string]) {
    const entry = this.get(id); entry.value = { ...entry.value, [key]: value }; entry.dirty = true;
    entry.listeners.forEach(fn => fn()); void this.flush(id);
  }
  async flush(id: string): Promise<void> {
    const entry = this.get(id); if (entry.pending) return entry.pending;
    entry.pending = (async () => {
      let rebased = false;
      while (entry.dirty) {
        const value = entry.value; entry.dirty = false;
        try { entry.version = await this.write(id, entry.version, value); entry.base = value; entry.error = false; }
        catch (cause) {
          entry.dirty = true;
          if (!rebased && String(cause).includes('UI state conflict')) {
            rebased = true;
            try {
              const remote = await this.read(id);
              const changes = Object.fromEntries(Object.entries(entry.value).filter(([key, value]) => value !== entry.base[key]));
              entry.base = remote.state; entry.version = remote.stateVersion;
              entry.value = { ...remote.state, ...changes }; continue;
            } catch { /* Preserve local edits if reloading also fails. */ }
          }
          entry.error = true; break;
        }
      }
    })();
    await entry.pending; entry.pending = undefined; entry.listeners.forEach(fn => fn()); this.prune();
  }
  private prune(protectedId?: string) {
    if (this.entries.size <= 150) return;
    for (const [id, entry] of this.entries) {
      if (id !== protectedId && !entry.owners && !entry.listeners.size && !entry.pending && !entry.dirty) this.entries.delete(id);
      if (this.entries.size <= 150) break;
    }
  }
}
export const uiStates = new InstanceStateStore();
