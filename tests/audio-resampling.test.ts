import { describe, expect, test } from "bun:test";
import { resamplePcm } from "../src/lib/audioResampling";

describe("audio resampling", () => {
  test("normalizes capture audio before IPC transfer", () => {
    const input = new Float32Array([0, 0.5, 1, 0.5, 0, -0.5]);
    expect(Array.from(resamplePcm(input, 6, 3))).toEqual([0, 1, 0]);
    expect(Array.from(resamplePcm(input, 6, 6))).toEqual(Array.from(input));
    expect(resamplePcm(input, 0, 16_000)).toHaveLength(0);
  });
});
