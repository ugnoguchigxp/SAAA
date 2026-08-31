import { describe, expect, test } from "bun:test";
import { STREAM_CHUNK_SIZE, StreamingTextBuffer } from "../src/features/chat/streamingTextBuffer";

describe("streaming text buffer", () => {
  test("keeps past text in immutable fixed-size chunks", () => {
    const buffer = new StreamingTextBuffer();
    buffer.append("a".repeat(STREAM_CHUNK_SIZE + 3));
    const first = buffer.snapshot();
    expect(first.chunks).toEqual(["a".repeat(STREAM_CHUNK_SIZE)]);
    expect(first.tail).toBe("aaa");

    buffer.append("b".repeat(STREAM_CHUNK_SIZE));
    const second = buffer.snapshot();
    expect(first).toEqual({
      chunks: ["a".repeat(STREAM_CHUNK_SIZE)],
      tail: "aaa",
      length: STREAM_CHUNK_SIZE + 3,
    });
    expect(second.chunks[0]).toBe(first.chunks[0]);
    expect(second.length).toBe(STREAM_CHUNK_SIZE * 2 + 3);
  });

  test("accepts 64,000 one-character deltas without rebuilding prior chunks", () => {
    const buffer = new StreamingTextBuffer();
    for (let index = 0; index < 64_000; index += 1) buffer.append("x");
    const projection = buffer.snapshot();
    expect(projection.length).toBe(64_000);
    expect(projection.chunks.length).toBe(Math.floor(64_000 / STREAM_CHUNK_SIZE));
    expect(projection.chunks.join("") + projection.tail).toBe("x".repeat(64_000));
  });

  test("never splits a Unicode surrogate pair between rendered chunks", () => {
    const buffer = new StreamingTextBuffer();
    const value = `${"a".repeat(STREAM_CHUNK_SIZE - 1)}😀b`;
    buffer.append(value);

    const projection = buffer.snapshot();
    expect(projection.chunks).toEqual(["a".repeat(STREAM_CHUNK_SIZE - 1)]);
    expect(projection.tail).toBe("😀b");
    expect(projection.chunks.join("") + projection.tail).toBe(value);
  });
});
