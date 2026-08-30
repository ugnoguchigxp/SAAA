import { describe, expect, test } from "bun:test";
import { encodePcm16 } from "../src/lib/audioIpc";

describe("binary audio IPC", () => {
  test("encodes bounded little-endian PCM16 without JSON number arrays", () => {
    const bytes = encodePcm16(new Float32Array([-2, -1, 0, 0.5, 1, 2]));
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    expect(bytes.byteLength).toBe(12);
    expect(Array.from({ length: 6 }, (_, index) => view.getInt16(index * 2, true))).toEqual([
      -32_767, -32_767, 0, 16_384, 32_767, 32_767,
    ]);
    expect(() => encodePcm16(new Float32Array([Number.NaN]))).toThrow("invalid samples");
  });
});
