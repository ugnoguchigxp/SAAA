import { describe, expect, test } from "bun:test";
import { SegmentQueue } from "../src/features/meeting/audio/segmentQueue";
import { appendFrames } from "../src/features/meeting/audio/pcm";

describe("meeting segment capture", () => {
  test("keeps a bounded two-segment queue without silently evicting audio", () => {
    const queue = new SegmentQueue<number>();
    expect(queue.push(1)).toBe(true);
    expect(queue.push(2)).toBe(true);
    expect(queue.push(3)).toBe(false);
    expect(queue.shift()).toBe(1);
    expect(queue.shift()).toBe(2);
  });

  test("preserves PCM frame order at segment boundaries", () => {
    expect([...appendFrames(new Float32Array([1]), [new Float32Array([2, 3]), new Float32Array([4])])]).toEqual([1, 2, 3, 4]);
  });
});
