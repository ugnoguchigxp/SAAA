import { readFileSync } from "node:fs";

import { parseRows, type Row } from "./conversation-context-input";
export { parseRows } from "./conversation-context-input";
const number = (value: unknown): value is number =>
  typeof value === "number" && Number.isFinite(value);
const count = (value: unknown): value is number =>
  number(value) && Number.isSafeInteger(value) && value >= 0;
const median = (values: number[]) => {
  const sorted = values.toSorted((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length === 0
    ? null
    : sorted.length % 2
      ? sorted[mid]
      : (sorted[mid - 1] + sorted[mid]) / 2;
};

export function summarize(rows: Row[]) {
  rows = [...new Map(rows.map((row) => [JSON.stringify(row), row])).values()];
  const groups = new Map<string, Row[]>();
  for (const row of rows.filter((row) => typeof row.mode === "string")) {
    const mode = String(row.mode);
    groups.set(mode, [...(groups.get(mode) ?? []), row]);
  }
  return [...groups].map(([mode, records]) => {
    const requests = records.filter((row) => number(row.omissionCount));
    const attempts = records.filter((row) => typeof row.usageStatus === "string");
    const pairedUsage = attempts.filter(
      (row) =>
        count(row.inputTokens) &&
        count(row.cacheReadTokens) &&
        row.cacheReadTokens <= row.inputTokens,
    );
    const input = pairedUsage.reduce((sum, row) => sum + (row.inputTokens as number), 0);
    const cached = pairedUsage.reduce((sum, row) => sum + (row.cacheReadTokens as number), 0);
    const completed = new Set(attempts.map((row) => row.logicalRequestId));
    const attemptIds = new Set(attempts.map((row) => row.httpAttemptId));
    const visible = rows.filter(
      (row) => attemptIds.has(row.httpAttemptId) && number(row.firstVisibleMs),
    );
    return {
      mode,
      logicalRequests: requests.length,
      httpAttempts: attempts.length,
      missingReceipts: requests.filter((row) => !completed.has(row.logicalRequestId)).length,
      fixedPrefixes: new Set(
        attempts.map((row) => row.fixedPrefixDigest).filter((value) => typeof value === "string"),
      ).size,
      usageProvided: attempts.filter((row) => row.usageStatus === "provider").length,
      cacheUsageMissing: attempts.filter((row) => !number(row.cacheReadTokens)).length,
      cacheExplicitZero: attempts.filter((row) => row.cacheReadTokens === 0).length,
      pairedUsageSamples: pairedUsage.length,
      invalidUsageSamples: attempts.filter(
        (row) =>
          number(row.inputTokens) && number(row.cacheReadTokens) && !pairedUsage.includes(row),
      ).length,
      cacheReadRatio: input > 0 ? cached / input : null,
      unsuccessfulAttempts: attempts.filter((row) => row.outcome !== "success").length,
      sseFirstContentMedianMs: median(
        attempts
          .filter((row) => row.requestMode === "sse" && number(row.firstContentMs))
          .map((row) => row.firstContentMs as number),
      ),
      firstVisibleMedianMs: median(visible.map((row) => row.firstVisibleMs as number)),
      completedMedianMs: median(
        attempts.filter((row) => number(row.completedMs)).map((row) => row.completedMs as number),
      ),
    };
  });
}

if (import.meta.main) {
  const paths = process.argv.slice(2);
  if (!paths.length)
    throw new Error("usage: bun scripts/conversation-context-report.ts observations.jsonl [...]");
  console.log(
    JSON.stringify(
      {
        measurement:
          "complete-message byte signatures; cache requires provider usage; timings are exploratory",
        results: summarize(paths.flatMap((path) => parseRows(readFileSync(path, "utf8")))),
      },
      null,
      2,
    ),
  );
}
