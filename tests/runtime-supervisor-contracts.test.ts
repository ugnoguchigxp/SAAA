import { describe, expect, test } from "bun:test";
import { runtimeFailureCodeSchema } from "../src/lib/schemas";

describe("runtime supervisor contracts", () => {
  test("accepts every bounded runtime failure code", () => {
    for (const code of [
      "runtime_error",
      "configuration-error",
      "child-start-failed",
      "request-timeout",
      "progress-timeout",
      "terminal-timeout",
      "hard-timeout",
      "child-exited",
      "protocol-error",
      "policy-violation",
      "provider-error",
      "response-too-large",
      "internal-error",
    ]) {
      expect(runtimeFailureCodeSchema.parse(code)).toBe(code);
    }
  });

  test("rejects unbounded provider failure values", () => {
    expect(() => runtimeFailureCodeSchema.parse("turn/failed: secret payload"))
      .toThrow();
  });
});
