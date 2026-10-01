import { expect, test } from "bun:test";
import { parseRows, summarize } from "../scripts/conversation-context-report";

test("missing usage is distinct from explicit zero and fallback remains two attempts", () => {
  const rows = parseRows(
    [
      { schemaVersion: 1, mode: "stable", logicalRequestId: "one", omissionCount: 0 },
      { schemaVersion: 1, mode: "stable", logicalRequestId: "two", omissionCount: 0 },
      {
        schemaVersion: 1,
        mode: "stable",
        logicalRequestId: "one",
        httpAttemptId: "sse",
        fixedPrefixDigest: "fixed",
        usageStatus: "disconnected",
        inputTokens: null,
        cacheReadTokens: null,
        requestMode: "sse",
        firstContentMs: null,
        completedMs: 1,
        outcome: "failure",
      },
      {
        schemaVersion: 1,
        mode: "stable",
        logicalRequestId: "one",
        httpAttemptId: "json",
        fixedPrefixDigest: "fixed",
        usageStatus: "provider",
        inputTokens: 10,
        cacheReadTokens: 0,
        requestMode: "json",
        firstContentMs: null,
        completedMs: 3,
        outcome: "success",
      },
    ]
      .map((row) => JSON.stringify(row))
      .join("\n"),
  );
  const [summary] = summarize(rows);
  expect(summary.httpAttempts).toBe(2);
  expect(summary.missingReceipts).toBe(1);
  expect(summary.cacheUsageMissing).toBe(1);
  expect(summary.cacheExplicitZero).toBe(1);
  expect(summary.cacheReadRatio).toBe(0);
  expect(summary.sseFirstContentMedianMs).toBeNull();
  expect(summary.unsuccessfulAttempts).toBe(1);
});

test("rejects unknown schemas and keeps unreported cache ratios null", () => {
  expect(() => parseRows('{"schemaVersion":2}')).toThrow();
  const [summary] = summarize([{ mode: "legacy", usageStatus: "missing", cacheReadTokens: null }]);
  expect(summary.cacheReadRatio).toBeNull();
});
