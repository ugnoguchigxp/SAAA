import { readFileSync, writeFileSync } from "node:fs";
import { z } from "zod";
const sampleSchema = z
  .object({
    condition: z.enum(["cold", "warm"]),
    mode: z.enum(["baseline", "personal-state"]),
    firstPlayableMs: z.number().nonnegative(),
    ttftMs: z.number().nonnegative(),
    finalMs: z.number().nonnegative(),
    workerWaitMs: z.number().nonnegative(),
    cancelSendMs: z.number().nonnegative(),
    cpuPercent: z.number().nonnegative(),
    tokensPerSecond: z.number().nonnegative(),
    sourceTokens: z.number().nonnegative(),
    sourceBytes: z.number().nonnegative(),
    snapshotBytes: z.number().nonnegative(),
    asrRtf: z.number().nonnegative(),
    ttsFirstPlayableMs: z.number().nonnegative(),
    ramBytes: z.number().nonnegative(),
    vramBytes: z.number().nonnegative(),
    concurrentGenerations: z.number().int().nonnegative(),
    success: z.boolean(),
    intentionalFault: z.boolean(),
  })
  .strict();
const inputSchema = z
  .object({
    release: z.string().min(1),
    limitsFixedBeforeRun: z.literal(true),
    maxCpuPercent: z.number().positive(),
    maxSourceTokens: z.number().positive().max(20_000_000),
    maxSourceBytes: z.number().positive(),
    maxSnapshotBytes: z.number().positive(),
    maxRamBytes: z.number().positive(),
    maxVramBytes: z.number().positive(),
    samples: z.array(sampleSchema),
  })
  .strict();
export function performanceReport(raw: unknown) {
  const input = inputSchema.parse(raw);
  const metrics = ["firstPlayableMs", "ttftMs", "finalMs", "workerWaitMs", "cancelSendMs"] as const;
  const quantile = (values: number[], q: number) => {
    const sorted = values.toSorted((a, b) => a - b);
    return sorted.length ? sorted[Math.ceil(q * sorted.length) - 1] : null;
  };
  const conditions = [];
  for (const condition of ["cold", "warm"] as const) {
    const groups = (["baseline", "personal-state"] as const).map((mode) => {
      const rows = input.samples.filter(
        (s) => s.condition === condition && s.mode === mode && !s.intentionalFault,
      );
      const latencies = Object.fromEntries(
        metrics.map((m) => [
          m,
          {
            p50: quantile(
              rows.map((s) => s[m]),
              0.5,
            ),
            p95: quantile(
              rows.map((s) => s[m]),
              0.95,
            ),
          },
        ]),
      );
      return {
        mode,
        count: rows.length,
        failures: rows.filter((s) => !s.success).length,
        latencies,
        observations: Object.fromEntries(
          (
            [
              "cpuPercent",
              "tokensPerSecond",
              "sourceTokens",
              "sourceBytes",
              "snapshotBytes",
              "asrRtf",
              "ttsFirstPlayableMs",
            ] as const
          ).map((m) => [
            m,
            {
              p50: quantile(
                rows.map((s) => s[m]),
                0.5,
              ),
              p95: quantile(
                rows.map((s) => s[m]),
                0.95,
              ),
            },
          ]),
        ),
        resourceViolations: rows.filter(
          (s) =>
            s.cpuPercent > input.maxCpuPercent ||
            s.sourceTokens > input.maxSourceTokens ||
            s.sourceBytes > input.maxSourceBytes ||
            s.snapshotBytes > input.maxSnapshotBytes ||
            s.ramBytes > input.maxRamBytes ||
            s.vramBytes > input.maxVramBytes ||
            s.concurrentGenerations > 1,
        ).length,
      };
    });
    const [baseline, enabled] = groups;
    const regression = ["firstPlayableMs", "ttftMs"].some((m) =>
      ["p50", "p95"].some((q) => {
        const b = baseline.latencies[m][q as "p50" | "p95"],
          e = enabled.latencies[m][q as "p50" | "p95"];
        return b === null || e === null || e > b * 1.1;
      }),
    );
    const cancelP95 = enabled.latencies.cancelSendMs.p95;
    conditions.push({
      condition,
      groups,
      passed:
        groups.every((g) => g.count >= 100 && g.failures === 0 && g.resourceViolations === 0) &&
        !regression &&
        cancelP95 !== null &&
        cancelP95 <= 100,
    });
  }
  return { release: input.release, conditions, passed: conditions.every((c) => c.passed) };
}
if (import.meta.main) {
  const input = process.argv[2];
  if (!input)
    throw new Error(
      "Usage: bun scripts/personal-state-performance.ts <samples.json> [report.json]",
    );
  const report = performanceReport(JSON.parse(readFileSync(input, "utf8")));
  const text = JSON.stringify(report, null, 2) + "\n";
  if (process.argv[3]) writeFileSync(process.argv[3], text);
  else process.stdout.write(text);
  if (!report.passed) process.exitCode = 1;
}
