import { describe, expect, test } from "bun:test";
import { SegmentQueue } from "../src/features/meeting/audio/segmentQueue";

describe("meeting segment capture", () => {
  test("keeps a bounded two-segment queue without silently evicting audio", () => {
    const queue = new SegmentQueue<number>();
    expect(queue.push(1)).toBe(true);
    expect(queue.push(2)).toBe(true);
    expect(queue.push(3)).toBe(false);
    expect(queue.shift()).toBe(1);
    expect(queue.shift()).toBe(2);
  });
});
