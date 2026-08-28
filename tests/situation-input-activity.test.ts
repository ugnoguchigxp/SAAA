import { describe, expect, test } from "bun:test";
import { calibrationParametersSchema, inputActivitySignalSchema } from "../src/lib/schemas";
import { decodeReplayMetrics } from "../src/features/situation/review/SituationReview";

describe("input activity contract", () => {
  test("accepts only bounded category and health", () => {
    expect(inputActivitySignalSchema.parse({ state: "idle", health: "ready" }))
      .toEqual({ state: "idle", health: "ready" });
    expect(inputActivitySignalSchema.parse({ state: "unknown", health: "unsupported" }))
      .toEqual({ state: "unknown", health: "unsupported" });
  });

  test("has no exact duration or last-input field", () => {
    expect(() => inputActivitySignalSchema.parse({
      state: "active",
      health: "ready",
      elapsedMs: 123,
    })).toThrow("Unrecognized key");
    expect(() => inputActivitySignalSchema.parse({
      state: "active",
      health: "ready",
      lastInputAt: "secret",
    })).toThrow("Unrecognized key");
  });

  test("validates candidate input-activity boundaries", () => {
    const candidate = {
      classificationMinConfidence: 70,
      lowConfidenceMax: 45,
      enterSampleCount: 3,
      exitSampleCount: 5,
      cooldownMs: 10_000,
      inputActiveMaxMs: 30_000,
      inputRecentMaxMs: 300_000,
    };
    expect(calibrationParametersSchema.parse(candidate)).toEqual(candidate);
    expect(() => calibrationParametersSchema.parse({
      ...candidate,
      inputActiveMaxMs: 300_000,
      inputRecentMaxMs: 300_000,
    })).toThrow("Input active boundary");
  });

  test("decodes replay attention metrics without unsafe defaults", () => {
    const metrics = {
      fixtureSetVersion: "situation-fixtures-v2",
      sampleCount: 17,
      expectedSceneMatches: 15,
      baselineExpectedSceneMatches: 15,
      expectedAttentionSamples: 14,
      expectedAttentionMatches: 14,
      baselineExpectedAttentionMatches: 14,
      shadowPolicyCounts: { ignore: 8, observe: 1, suggest: 7, respond: 1 },
    };
    expect(decodeReplayMetrics(JSON.stringify(metrics))).toEqual(metrics);
    expect(decodeReplayMetrics(JSON.stringify({
      ...metrics,
      expectedAttentionMatches: 15,
    }))).toBeNull();
  });
});
