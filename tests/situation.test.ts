import { describe, expect, test } from "bun:test";
import { situationSettingsSchema } from "../src/lib/schemas";

const valid = {
  enabled: false,
  sampleIntervalMs: 2_000,
  calendarEnabled: false,
  retentionDays: 7,
  maxLedgerEntries: 10_000,
  heartbeatIntervalMs: 300_000,
  sensitiveApplicationCategories: true as const,
};

describe("Situation settings contract", () => {
  test("accepts privacy-preserving bounded defaults", () => {
    expect(situationSettingsSchema.parse(valid)).toEqual(valid);
  });

  test("rejects unbounded sampling, retention, and privacy relaxation", () => {
    expect(() => situationSettingsSchema.parse({ ...valid, sampleIntervalMs: 100 })).toThrow();
    expect(() => situationSettingsSchema.parse({ ...valid, retentionDays: 365 })).toThrow();
    expect(() => situationSettingsSchema.parse({ ...valid, sensitiveApplicationCategories: false })).toThrow();
  });
});
