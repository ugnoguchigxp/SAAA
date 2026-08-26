export class SegmentQueue<T> {
  private values: T[] = [];
  push(value: T): boolean { if (this.values.length >= 2) return false; this.values.push(value); return true; }
  shift(): T | undefined { return this.values.shift(); }
  clear(): void { this.values = []; }
  get length(): number { return this.values.length; }
}
