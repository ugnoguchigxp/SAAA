import { expect, test } from "bun:test";
import { summarize } from "../scripts/conversation-context-report";

test("deduplicates exports and excludes impossible token ratios", () => {
  const valid = {
    mode: "stable",
    usageStatus: "provider",
    inputTokens: 10,
    cacheReadTokens: 5,
    httpAttemptId: "valid",
  };
  const [summary] = summarize([
    valid,
    valid,
    { ...valid, cacheReadTokens: 20, httpAttemptId: "bad" },
    { ...valid, inputTokens: -1, cacheReadTokens: 0, httpAttemptId: "negative" },
  ]);
  expect(summary.httpAttempts).toBe(3);
  expect(summary.pairedUsageSamples).toBe(1);
  expect(summary.invalidUsageSamples).toBe(2);
  expect(summary.cacheReadRatio).toBe(0.5);
  expect(summary.fixedPrefixes).toBe(0);
});
