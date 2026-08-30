import { describe, expect, test } from "bun:test";
import {
  VOICE_ENROLLMENT_AUTO_STOP_MS,
  VOICE_ENROLLMENT_MINIMUM_SECONDS,
  VOICE_ENROLLMENT_PROMPTS,
} from "../src/features/settings/voiceEnrollmentPrompts";

describe("voice enrollment prompts", () => {
  test("collects five long prompts with varied Japanese intonation", () => {
    expect(VOICE_ENROLLMENT_PROMPTS).toHaveLength(5);
    expect(new Set(VOICE_ENROLLMENT_PROMPTS).size).toBe(5);
    expect(VOICE_ENROLLMENT_PROMPTS.every((prompt) => prompt.length >= 140)).toBe(true);
    expect(VOICE_ENROLLMENT_PROMPTS.every((prompt) => prompt.length <= 220)).toBe(true);
    expect(VOICE_ENROLLMENT_PROMPTS.some((prompt) => prompt.includes("？"))).toBe(true);
    expect(VOICE_ENROLLMENT_PROMPTS.some((prompt) => prompt.includes("！"))).toBe(true);
    expect(VOICE_ENROLLMENT_MINIMUM_SECONDS).toBe(10);
    expect(VOICE_ENROLLMENT_AUTO_STOP_MS).toBeGreaterThanOrEqual(10_000);
    expect(VOICE_ENROLLMENT_AUTO_STOP_MS).toBeLessThan(12_000);
  });
});
