import { describe, expect, test } from "bun:test";
import { regionalPreferencesSchema } from "../src/lib/schemas";

const valid = {
  language: "ja",
  timeZone: "Asia/Tokyo",
  lengthUnit: "metric",
  weightUnit: "kilogram",
  currency: "JPY",
};

describe("regional preference settings", () => {
  test("accepts supported values and rejects unknown values", () => {
    expect(() => regionalPreferencesSchema.parse(valid)).not.toThrow();
    for (const [key, value] of [["language", "fr"], ["timeZone", "not a zone"], ["lengthUnit", "nautical"], ["weightUnit", "stone"], ["currency", "BTC"]]) {
      expect(() => regionalPreferencesSchema.parse({ ...valid, [key]: value })).toThrow();
    }
  });
});
