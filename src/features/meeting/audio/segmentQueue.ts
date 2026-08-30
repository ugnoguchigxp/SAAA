export class SegmentQueue<T> {
  private values: T[] = [];
  constructor(private readonly capacity = 2, private readonly discard?: (value: T) => void) {}
  push(value: T): boolean { if (this.values.length >= this.capacity) { this.discard?.(value); return false; } this.values.push(value); return true; }
  shift(): T | undefined { return this.values.shift(); }
  clear(): void { for (const value of this.values) this.discard?.(value); this.values = []; }
  get length(): number { return this.values.length; }
}
