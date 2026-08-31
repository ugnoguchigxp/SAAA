import { describe, expect, test } from "bun:test";
import { StatefulMeetingResampler } from "../src/features/meeting/audio/meetingAudioResampler";

function normalize(input: Float32Array, chunkSize: number) {
  const segments: Float32Array[] = [];
  const resampler = new StatefulMeetingResampler(48_000, 0, (segment) => {
    segments.push(segment.samples);
  });
  for (let offset = 0; offset < input.length; offset += chunkSize) {
    resampler.append(input.slice(offset, offset + chunkSize));
  }
  resampler.flush();
  resampler.flush();
  const length = segments.reduce((total, segment) => total + segment.length, 0);
  const output = new Float32Array(length);
  let offset = 0;
  for (const segment of segments) {
    output.set(segment, offset);
    offset += segment.length;
  }
  return { segments, output };
}

describe("meeting audio worker resampler", () => {
  test("preserves phase and exact sample count across 100 ms input chunks", () => {
    const input = Float32Array.from({ length: 48_000 * 2 }, (_, index) =>
      Math.sin(2 * Math.PI * 997 * index / 48_000));
    const chunked = normalize(input, 4_800);
    const whole = normalize(input, input.length);
    expect(chunked.output.length).toBe(32_000);
    expect(chunked.segments.map((segment) => segment.length)).toEqual([16_000, 16_000]);
    let maximumDifference = 0;
    for (let index = 0; index < chunked.output.length; index += 1) {
      maximumDifference = Math.max(maximumDifference, Math.abs(chunked.output[index] - whole.output[index]));
    }
    expect(maximumDifference).toBeLessThan(1e-6);
  });

  test("clips normalized output and suppresses aliases", () => {
    const clipped = normalize(new Float32Array(48_000).fill(4), 4_800).output;
    expect(clipped.every((sample) => sample >= -1 && sample <= 1)).toBe(true);
    const alias = normalize(Float32Array.from({ length: 48_000 }, (_, index) =>
      Math.sin(2 * Math.PI * 12_000 * index / 48_000)), 4_800).output.slice(64, -64);
    const rms = Math.sqrt(alias.reduce((sum, sample) => sum + sample * sample, 0) / alias.length);
    expect(rms).toBeLessThan(0.03);
  });

  test("flush is one-shot and emitted segments own distinct buffers", () => {
    const result = normalize(new Float32Array(48_000).fill(0.25), 4_800);
    expect(result.segments).toHaveLength(1);
    expect(result.segments[0].buffer).not.toBe(result.output.buffer);
  });
});
