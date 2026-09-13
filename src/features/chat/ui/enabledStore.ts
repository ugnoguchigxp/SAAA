/** A late initial read must not overwrite a successfully persisted user toggle. */
export class UiEnabledStore {
  private value = false;
  private revision = 0;
  private loading?: Promise<void>;
  private writes: Promise<void> = Promise.resolve();
  private listeners = new Set<() => void>();
  constructor(
    private read: () => Promise<boolean>,
    private write: (value: boolean) => Promise<void>,
  ) {}
  snapshot = () => this.value;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  private publish(value: boolean) {
    this.value = value;
    this.listeners.forEach((fn) => fn());
  }
  load = () => {
    if (!this.loading) {
      const revision = this.revision;
      this.loading = this.read()
        .then((value) => {
          if (revision === this.revision) this.publish(value);
        })
        .catch(() => {
          this.loading = undefined;
        });
    }
    return this.loading;
  };
  set = (value: boolean) => {
    const result = this.writes.then(async () => {
      await this.write(value);
      this.revision++;
      this.publish(value);
    });
    this.writes = result.catch(() => {});
    return result;
  };
}
