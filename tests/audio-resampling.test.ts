import { describe, expect, test } from "bun:test";
import { resamplePcm } from "../src/lib/audioResampling";

describe("audio resampling", () => {
  test("preserves speech-band energy while suppressing downsampling aliases", () => {
    const sourceRate = 48_000;
    const targetRate = 16_000;
    const sine = (frequency: number) => Float32Array.from(
      { length: sourceRate / 5 },
      (_, index) => Math.sin(2 * Math.PI * frequency * index / sourceRate),
    );
    const rms = (values: Float32Array) => {
      const stable = values.slice(32, -32);
      return Math.sqrt(stable.reduce((sum, value) => sum + value * value, 0) / stable.length);
    };
    expect(rms(resamplePcm(sine(1_000), sourceRate, targetRate))).toBeGreaterThan(0.65);
    expect(rms(resamplePcm(sine(12_000), sourceRate, targetRate))).toBeLessThan(0.03);
  });

  test("copies equal-rate PCM and rejects implausible rates", () => {
    const input = new Float32Array([0, 0.5, 1, 0.5, 0, -0.5]);
    expect(Array.from(resamplePcm(input, 16_000, 16_000))).toEqual(Array.from(input));
    expect(resamplePcm(input, 0, 16_000)).toHaveLength(0);
  });
});
