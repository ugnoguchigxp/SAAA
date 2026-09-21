import { describe, expect, test } from "bun:test";
import {
  isRetryableRequiredContextFailure,
  requiredContextFailureCode,
} from "../src/features/chat/requiredContextRecovery";

describe("required context recovery", () => {
  test("keeps the three refusal reasons distinct and non-retryable", () => {
    for (const code of [
      "required-context-overflow",
      "context-scope-changed",
      "required-context-unavailable",
    ] as const) {
      expect(requiredContextFailureCode(code)).toBe(code);
      expect(isRetryableRequiredContextFailure(code)).toBeFalse();
    }
  });

  test("does not turn ordinary provider failures into recovery refusals", () => {
    expect(requiredContextFailureCode("provider-error")).toBeNull();
    expect(isRetryableRequiredContextFailure("provider-error")).toBeTrue();
  });
});
