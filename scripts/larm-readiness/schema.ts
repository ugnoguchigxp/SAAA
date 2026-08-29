import { fileURLToPath } from "node:url";
import { isAbsolute, join } from "node:path";
import { z } from "zod";

export const REPORT_FORMAT = "saaa-larm-readiness-v1" as const;
export const MANIFEST_FORMAT = "saaa-larm-canary-manifest-v1" as const;
export const REPORT_FILENAMES = {
  preflight: "preflight.json",
  functional: "functional.json",
  "soak-30m": "soak-30m.json",
  "soak-2h": "soak-2h.json",
  aggregate: "aggregate.json",
} as const;
export const RUST_FRAGMENT_FILENAMES = {
  preflight: ".preflight-rust.json",
  functional: ".functional-rust.json",
  "soak-30m": ".soak-30m-rust.json",
  "soak-2h": ".soak-2h-rust.json",
} as const;

// G1 is deliberately closed while the three deviations recorded in release evidence remain open.
// Replace null with the reviewed LARM commit in the same change that updates the production contract.
export const FROZEN_LARM_CONTRACT_COMMIT: string | null = null;
export const COMPILED_LARM_CONTRACT_COMMIT = "7dca7c3";
export const RESIDENT_DEFAULT_RUNTIME_IDS = new Set(["qwen-general"]);
export const OPTIONAL_RUNTIME_IDS = new Set<string>();

export const ROOT = fileURLToPath(new URL("../..", import.meta.url));
export const RELEASE_BUNDLE = join(ROOT, "src-tauri/target/release/bundle/macos/SAAA.app");
export const RELEASE_EXECUTABLE = join(RELEASE_BUNDLE, "Contents/MacOS/saaa");
export const MAX_REPORT_BYTES = 64 * 1024;
export const MAX_MANIFEST_BYTES = 16 * 1024;
export const MAX_CHILD_BYTES = 1024 * 1024;
export const MAX_BUILD_BYTES = 8 * 1024 * 1024;

export const FAILURE_CODES = [
  "gate-missing",
  "environment-invalid",
  "artifact-mismatch",
  "contract-mismatch",
  "authentication-failed",
  "health-failed",
  "ready-failed",
  "allocation-failed",
  "gateway-failed",
  "cancel-failed",
  "partial-output-violation",
  "restart-recovery-failed",
  "capacity-policy-failed",
  "ttl-recovery-failed",
  "rollback-failed",
  "sampling-gap",
  "rss-growth",
  "allocation-leak",
  "runtime-policy-violation",
  "redaction-failed",
  "database-schema-invalid",
  "report-schema-invalid",
  "runner-timeout",
  "internal",
] as const;

export const SCENARIO_KEYS = [
  "normalTurns",
  "cancellations",
  "requestTimeouts",
  "partialInterruptions",
  "larmRestarts",
  "saaaRestarts",
  "capacityRejections",
  "ttlRecoveries",
  "renewals",
  "rollbackPreflightTurns",
  "settingsRollbackTurns",
  "killSwitchRollbackTurns",
] as const;

export const RESULT_KEYS = [
  "completed",
  "cancelled",
  "expectedFailures",
  "unexpectedFailures",
  "duplicateTerminals",
  "explicitProviderFallbacks",
  "implicitFallbacks",
  "staleAllocationReuses",
  "runtimePolicyViolations",
  "leakedAllocations",
] as const;

export const TIMING_KEYS = [
  "elapsedMs",
  "sampleIntervalSeconds",
  "rssMaxSamplingGapSeconds",
  "metricsMaxSamplingGapSeconds",
  "plannedLarmRestartGapSeconds",
  "releaseRecoveryMaxMs",
  "ttlRecoveryMaxMs",
] as const;

export const RESOURCE_KEYS = [
  "baselineActiveAllocations",
  "maxActiveAllocations",
  "finalActiveAllocations",
  "rssRangeMiB",
  "rssPrevious30mMedianMiB",
  "rssLast30mMedianMiB",
] as const;

export const LEASE_KEYS = [
  "effectiveTtlSecondsMin",
  "effectiveTtlSecondsMax",
  "renewalsAttempted",
  "renewalsSucceeded",
] as const;

export const count = z.number().int().min(0).max(10_000);
export const milliseconds = z.number().int().min(0).max(10_800_000);
export const seconds = z.number().int().min(0).max(10_800);
export const rss = z.number().int().finite().min(0).max(1_048_576);
export const commit = z.string().regex(/^[0-9a-f]{7,64}$/);
export const sha256 = z.string().regex(/^[0-9a-f]{64}$/);
export const revision = z.string().regex(/^[A-Za-z0-9._-]{1,64}$/);
export const utcTimestamp = z.string().refine((value) => {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d{1,9})?Z$/.exec(value);
  if (match === null) return false;
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) return false;
  const date = new Date(parsed);
  return date.getUTCFullYear() === Number(match[1])
    && date.getUTCMonth() + 1 === Number(match[2])
    && date.getUTCDate() === Number(match[3])
    && date.getUTCHours() === Number(match[4])
    && date.getUTCMinutes() === Number(match[5])
    && date.getUTCSeconds() === Number(match[6]);
}, "expected a UTC RFC 3339 timestamp");

export function utcTimestampSortKey(value: string): string {
  const [whole, fraction = ""] = value.slice(0, -1).split(".", 2);
  return `${whole}.${fraction.padEnd(9, "0")}`;
}

export const scenarioSchema = z.object(Object.fromEntries(SCENARIO_KEYS.map((key) => [key, count])) as Record<typeof SCENARIO_KEYS[number], typeof count>).strict();
export const resultCountSchema = z.object(Object.fromEntries(RESULT_KEYS.map((key) => [key, count])) as Record<typeof RESULT_KEYS[number], typeof count>).strict();
export const timingSchema = z.object({
  elapsedMs: milliseconds,
  sampleIntervalSeconds: seconds,
  rssMaxSamplingGapSeconds: seconds,
  metricsMaxSamplingGapSeconds: seconds,
  plannedLarmRestartGapSeconds: seconds,
  releaseRecoveryMaxMs: milliseconds,
  ttlRecoveryMaxMs: milliseconds,
}).strict();
export const resourceSchema = z.object({
  baselineActiveAllocations: count,
  maxActiveAllocations: count,
  finalActiveAllocations: count,
  rssRangeMiB: rss,
  rssPrevious30mMedianMiB: rss,
  rssLast30mMedianMiB: rss,
}).strict();
export const leaseSchema = z.object({
  effectiveTtlSecondsMin: z.number().int().min(0).max(3_600),
  effectiveTtlSecondsMax: z.number().int().min(0).max(3_600),
  renewalsAttempted: count,
  renewalsSucceeded: count,
}).strict();

export const readinessReportSchema = z.object({
  format: z.literal(REPORT_FORMAT),
  saaaCommit: commit,
  saaaArtifactSha256: sha256,
  canaryManifestSha256: sha256,
  larmContractCommit: commit,
  deploymentRevision: revision,
  startedAt: utcTimestamp,
  finishedAt: utcTimestamp,
  mode: z.enum(["preflight", "functional", "soak-30m", "soak-2h", "aggregate"]),
  scenarioCounts: scenarioSchema,
  resultCounts: resultCountSchema,
  timingSummary: timingSchema,
  resourceSummary: resourceSchema,
  leaseSummary: leaseSchema,
  failureCodes: z.array(z.enum(FAILURE_CODES)).max(32).refine((values) => new Set(values).size === values.length, "duplicate failure code"),
  redactionCheck: z.enum(["passed", "failed"]),
  result: z.enum(["passed", "failed", "blocked"]),
}).strict().superRefine((report, context) => {
  if (utcTimestampSortKey(report.finishedAt) < utcTimestampSortKey(report.startedAt)) {
    context.addIssue({ code: "custom", path: ["finishedAt"], message: "finishedAt precedes startedAt" });
  }
  const ttlValues = [report.leaseSummary.effectiveTtlSecondsMin, report.leaseSummary.effectiveTtlSecondsMax];
  if ((report.mode === "functional" || report.mode === "aggregate") && report.result === "passed") {
    if (ttlValues.some((value) => value === 0) || ttlValues[0] > ttlValues[1]) {
      context.addIssue({ code: "custom", path: ["leaseSummary"], message: "invalid functional TTL range" });
    }
  } else if (report.mode === "functional" || report.mode === "aggregate") {
    if (!((ttlValues[0] === 0 && ttlValues[1] === 0) || (ttlValues[0] > 0 && ttlValues[0] <= ttlValues[1]))) {
      context.addIssue({ code: "custom", path: ["leaseSummary"], message: "invalid unavailable TTL range" });
    }
  } else if (ttlValues.some((value) => value !== 0) || report.leaseSummary.renewalsAttempted !== 0 || report.leaseSummary.renewalsSucceeded !== 0) {
    context.addIssue({ code: "custom", path: ["leaseSummary"], message: "lease summary must be zero for this mode" });
  }
  if (report.result === "passed" && (report.failureCodes.length !== 0 || report.redactionCheck !== "passed")) {
    context.addIssue({ code: "custom", path: ["result"], message: "passed report contains failures" });
  }
  if (report.result !== "passed" && report.failureCodes.length === 0) {
    context.addIssue({ code: "custom", path: ["failureCodes"], message: "non-passed report requires a failure code" });
  }
  if ((report.redactionCheck === "failed") !== report.failureCodes.includes("redaction-failed")) {
    context.addIssue({ code: "custom", path: ["redactionCheck"], message: "redaction result and failure code disagree" });
  }
  if (report.result === "blocked" && report.failureCodes.some((code) => !["gate-missing", "environment-invalid"].includes(code))) {
    context.addIssue({ code: "custom", path: ["failureCodes"], message: "blocked report contains a failure-only code" });
  }
});

export const manifestSchema = z.object({
  format: z.literal(MANIFEST_FORMAT),
  saaaCommit: commit,
  larmContractCommit: commit,
  deploymentRevision: revision,
  dataDirectory: z.string().min(1).max(4_096),
  metricsScope: z.enum(["exclusive-window", "client-scoped"]),
  larmProvider: z.object({
    baseUrl: z.string().min(1).max(2_048),
    allocationTtlSeconds: z.literal(300),
    allocationStartupTimeoutSeconds: z.literal(300),
    allowFallbackByDefault: z.literal(false),
    deploymentPolicy: z.literal("existing-only"),
  }).strict(),
  rollbackProvider: z.object({
    id: z.literal("local-openai-compatible"),
    location: z.enum(["local", "cloud"]),
    endpoint: z.string().min(1).max(2_048),
    model: z.string().trim().min(1).max(240),
    credentialEnv: z.literal("SAAA_PROVIDER_LOCAL_OPENAI_COMPATIBLE_API_KEY"),
    credentialRequired: z.boolean(),
  }).strict(),
}).strict();

export type ReadinessReport = z.infer<typeof readinessReportSchema>;
export type CanaryManifest = z.infer<typeof manifestSchema>;
export type ReportMode = ReadinessReport["mode"];
export type LiveMode = Exclude<ReportMode, "aggregate">;
export type Result = ReadinessReport["result"];

export interface CliArguments {
  command: "preflight" | "canary" | "soak" | "report";
  reportDirectory: string;
  duration?: "30m" | "2h";
}

export class RunnerError extends Error {
  constructor(
    readonly exitCode: 2 | 3 | 64 | 70,
    readonly errorCode: string,
    readonly result: "failed" | "blocked",
  ) {
    super(errorCode);
  }
}

export function usage(): never {
  throw new RunnerError(64, "usage-error", "failed");
}

export function parseCliArguments(argv: string[]): CliArguments {
  if (argv.length < 1 || !["preflight", "canary", "soak", "report"].includes(argv[0]!)) usage();
  const command = argv[0] as CliArguments["command"];
  let reportDirectory: string | undefined;
  let duration: "30m" | "2h" | undefined;
  for (let index = 1; index < argv.length; index += 1) {
    const argument = argv[index];
    const value = argv[index + 1];
    if (argument === "--report-dir" && reportDirectory === undefined && value !== undefined && !value.startsWith("--")) {
      reportDirectory = value;
      index += 1;
    } else if (argument === "--duration" && duration === undefined && (value === "30m" || value === "2h")) {
      duration = value;
      index += 1;
    } else {
      usage();
    }
  }
  if (reportDirectory === undefined || !isAbsolute(reportDirectory)) usage();
  if ((command === "soak") !== (duration !== undefined)) usage();
  return { command, reportDirectory, ...(duration === undefined ? {} : { duration }) };
}

export function validateNumericLoopbackOrigin(value: string): string {
  let parsed: URL;
  try {
    parsed = new URL(value);
  } catch {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
  const hostname = parsed.hostname.toLowerCase();
  const ipv4 = hostname.split(".");
  const ipv4Loopback = ipv4.length === 4
    && ipv4.every((part) => /^\d{1,3}$/.test(part) && Number(part) <= 255)
    && Number(ipv4[0]) === 127;
  const ipv6Loopback = hostname === "[::1]";
  if (
    parsed.protocol !== "http:"
    || (!ipv4Loopback && !ipv6Loopback)
    || parsed.port === ""
    || parsed.username !== ""
    || parsed.password !== ""
    || parsed.pathname !== "/"
    || parsed.search !== ""
    || parsed.hash !== ""
    || value.endsWith("/")
    || parsed.origin !== value
  ) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
  return value;
}

export function validateHeaderCredential(value: string | undefined): string {
  if (value === undefined) throw new RunnerError(3, "gate-missing", "blocked");
  const bytes = Buffer.byteLength(value, "utf8");
  if (bytes < 1 || bytes > 4_096 || !/^[\x21-\x7e]+$/.test(value)) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
  return value;
}

export function zeroRecord<const T extends readonly string[]>(keys: T): Record<T[number], number> {
  return Object.fromEntries(keys.map((key) => [key, 0])) as Record<T[number], number>;
}

export function emptyReport(identity: {
  saaaCommit: string;
  artifactSha256: string;
  manifestSha256: string;
  larmContractCommit: string;
  deploymentRevision: string;
}, mode: LiveMode, result: Result = "passed", failureCodes: ReadinessReport["failureCodes"] = []): ReadinessReport {
  const now = new Date().toISOString();
  return {
    format: REPORT_FORMAT,
    saaaCommit: identity.saaaCommit,
    saaaArtifactSha256: identity.artifactSha256,
    canaryManifestSha256: identity.manifestSha256,
    larmContractCommit: identity.larmContractCommit,
    deploymentRevision: identity.deploymentRevision,
    startedAt: now,
    finishedAt: now,
    mode,
    scenarioCounts: zeroRecord(SCENARIO_KEYS),
    resultCounts: zeroRecord(RESULT_KEYS),
    timingSummary: zeroRecord(TIMING_KEYS),
    resourceSummary: zeroRecord(RESOURCE_KEYS),
    leaseSummary: zeroRecord(LEASE_KEYS),
    failureCodes,
    redactionCheck: failureCodes.includes("redaction-failed") ? "failed" : "passed",
    result,
  };
}

export function validateReport(value: unknown): ReadinessReport {
  let json: string | undefined;
  try {
    json = JSON.stringify(value);
  } catch {
    throw new RunnerError(2, "report-schema-invalid", "failed");
  }
  if (json === undefined) throw new RunnerError(2, "report-schema-invalid", "failed");
  const encoded = Buffer.from(json);
  if (encoded.length > MAX_REPORT_BYTES) throw new RunnerError(2, "report-schema-invalid", "failed");
  const parsed = readinessReportSchema.safeParse(value);
  if (!parsed.success) throw new RunnerError(2, "report-schema-invalid", "failed");
  return parsed.data;
}

export function evaluateReport(reportInput: ReadinessReport): ReadinessReport {
  const report = validateReport(reportInput);
  if (report.result !== "passed") return report;
  const allZero = (record: Record<string, number>) => Object.values(record).every((value) => value === 0);
  const failureCodes: ReadinessReport["failureCodes"] = [];
  if (report.mode === "preflight") {
    if (!allZero(report.scenarioCounts) || !allZero(report.resultCounts)) failureCodes.push("report-schema-invalid");
  } else if (report.mode === "functional") {
    const requiredScenarios: ReadinessReport["scenarioCounts"] = {
      normalTurns: 5,
      cancellations: 2,
      requestTimeouts: 1,
      partialInterruptions: 1,
      larmRestarts: 1,
      saaaRestarts: 1,
      capacityRejections: 1,
      ttlRecoveries: 3,
      renewals: 1,
      rollbackPreflightTurns: 1,
      settingsRollbackTurns: 3,
      killSwitchRollbackTurns: 3,
    };
    const requiredResults: ReadinessReport["resultCounts"] = {
      completed: 14,
      cancelled: 2,
      expectedFailures: 3,
      unexpectedFailures: 0,
      duplicateTerminals: 0,
      explicitProviderFallbacks: 2,
      implicitFallbacks: 0,
      staleAllocationReuses: 0,
      runtimePolicyViolations: 0,
      leakedAllocations: 0,
    };
    if (
      !SCENARIO_KEYS.every((key) => report.scenarioCounts[key] === requiredScenarios[key])
      || !RESULT_KEYS.every((key) => report.resultCounts[key] === requiredResults[key])
    ) failureCodes.push("report-schema-invalid");
    const leaseValid = report.leaseSummary.effectiveTtlSecondsMin >= 60
      && report.leaseSummary.effectiveTtlSecondsMax <= 300
      && report.leaseSummary.renewalsAttempted === 1
      && report.leaseSummary.renewalsSucceeded === 1
      && report.timingSummary.ttlRecoveryMaxMs <= report.leaseSummary.effectiveTtlSecondsMax * 1_000 + 30_000;
    if (!leaseValid) failureCodes.push("ttl-recovery-failed");
    if (report.timingSummary.releaseRecoveryMaxMs > 10_000) failureCodes.push("allocation-leak");
  } else if (report.mode === "soak-30m" || report.mode === "soak-2h") {
    const minimumNormal = report.mode === "soak-30m" ? 20 : 60;
    const minimumCancel = report.mode === "soak-30m" ? 5 : 10;
    const workloadValid = report.scenarioCounts.normalTurns >= minimumNormal
      && report.scenarioCounts.cancellations >= minimumCancel
      && report.resultCounts.completed === report.scenarioCounts.normalTurns
      && report.resultCounts.cancelled === report.scenarioCounts.cancellations
      && RESULT_KEYS.filter((key) => key !== "completed" && key !== "cancelled").every((key) => report.resultCounts[key] === 0)
      && SCENARIO_KEYS.filter((key) => !["normalTurns", "cancellations", "larmRestarts", "saaaRestarts"].includes(key)).every((key) => report.scenarioCounts[key] === 0)
      && (report.mode === "soak-30m"
        ? report.scenarioCounts.larmRestarts === 0 && report.scenarioCounts.saaaRestarts === 0
        : report.scenarioCounts.larmRestarts === 1 && report.scenarioCounts.saaaRestarts === 1);
    if (!workloadValid) failureCodes.push("report-schema-invalid");
    const samplingValid = report.timingSummary.sampleIntervalSeconds === 5
      && report.timingSummary.rssMaxSamplingGapSeconds <= 15
      && report.timingSummary.metricsMaxSamplingGapSeconds <= 15
      && (report.mode === "soak-30m"
        ? report.timingSummary.elapsedMs >= 1_800_000
          && report.timingSummary.plannedLarmRestartGapSeconds === 0
        : report.timingSummary.elapsedMs >= 7_200_000
          && report.timingSummary.plannedLarmRestartGapSeconds >= 1
          && report.timingSummary.plannedLarmRestartGapSeconds <= 120);
    if (!samplingValid) failureCodes.push("sampling-gap");
    const rssValid = report.resourceSummary.rssRangeMiB <= 64
      && (report.mode === "soak-30m"
        ? report.resourceSummary.rssPrevious30mMedianMiB === 0
          && report.resourceSummary.rssLast30mMedianMiB === 0
        : report.resourceSummary.rssLast30mMedianMiB <= report.resourceSummary.rssPrevious30mMedianMiB + 16);
    if (!rssValid) failureCodes.push("rss-growth");
    if (report.timingSummary.releaseRecoveryMaxMs > 10_000) failureCodes.push("allocation-leak");
  }
  const resourcesValid = report.resourceSummary.baselineActiveAllocations === 0
    && report.resourceSummary.finalActiveAllocations === 0
    && (report.mode === "preflight"
      ? report.resourceSummary.maxActiveAllocations === 0
      : report.resourceSummary.maxActiveAllocations <= 1);
  if (!resourcesValid) failureCodes.push("allocation-leak");
  if (failureCodes.length === 0) return report;
  return validateReport({
    ...report,
    result: "failed",
    failureCodes: [...new Set(failureCodes)],
  });
}

export function reportFailure(report: ReadinessReport): RunnerError | undefined {
  if (report.result === "passed") return undefined;
  const code = report.failureCodes[0] ?? "internal";
  return new RunnerError(report.result === "blocked" ? 3 : 2, code, report.result);
}

export function mergeReports(rustInput: ReadinessReport, localInput: ReadinessReport): ReadinessReport {
  const rust = validateReport(rustInput);
  const local = validateReport(localInput);
  for (const key of ["saaaCommit", "saaaArtifactSha256", "canaryManifestSha256", "larmContractCommit", "deploymentRevision", "mode"] as const) {
    if (rust[key] !== local[key]) throw new RunnerError(2, "report-schema-invalid", "failed");
  }
  if (rust.resourceSummary.rssRangeMiB !== 0 || rust.resourceSummary.rssPrevious30mMedianMiB !== 0 || rust.resourceSummary.rssLast30mMedianMiB !== 0) {
    throw new RunnerError(2, "report-schema-invalid", "failed");
  }
  if (local.resourceSummary.baselineActiveAllocations !== 0 || local.resourceSummary.maxActiveAllocations !== 0 || local.resourceSummary.finalActiveAllocations !== 0 || Object.values(local.leaseSummary).some((value) => value !== 0)) {
    throw new RunnerError(2, "report-schema-invalid", "failed");
  }
  const add = <T extends readonly string[]>(keys: T, left: Record<T[number], number>, right: Record<T[number], number>) => Object.fromEntries(keys.map((key) => {
    const value = left[key] + right[key];
    if (!Number.isSafeInteger(value) || value > 10_800_000) throw new RunnerError(2, "report-schema-invalid", "failed");
    return [key, value];
  })) as Record<T[number], number>;
  const strength = { passed: 0, blocked: 1, failed: 2 } as const;
  const result = strength[rust.result] >= strength[local.result] ? rust.result : local.result;
  const merged: ReadinessReport = {
    ...rust,
    startedAt: utcTimestampSortKey(rust.startedAt) <= utcTimestampSortKey(local.startedAt) ? rust.startedAt : local.startedAt,
    finishedAt: utcTimestampSortKey(rust.finishedAt) >= utcTimestampSortKey(local.finishedAt) ? rust.finishedAt : local.finishedAt,
    scenarioCounts: add(SCENARIO_KEYS, rust.scenarioCounts, local.scenarioCounts),
    resultCounts: add(RESULT_KEYS, rust.resultCounts, local.resultCounts),
    timingSummary: Object.fromEntries(TIMING_KEYS.map((key) => [key, Math.max(rust.timingSummary[key], local.timingSummary[key])])) as ReadinessReport["timingSummary"],
    resourceSummary: {
      baselineActiveAllocations: rust.resourceSummary.baselineActiveAllocations,
      maxActiveAllocations: rust.resourceSummary.maxActiveAllocations,
      finalActiveAllocations: rust.resourceSummary.finalActiveAllocations,
      rssRangeMiB: local.resourceSummary.rssRangeMiB,
      rssPrevious30mMedianMiB: local.resourceSummary.rssPrevious30mMedianMiB,
      rssLast30mMedianMiB: local.resourceSummary.rssLast30mMedianMiB,
    },
    leaseSummary: rust.leaseSummary,
    failureCodes: [...new Set([...rust.failureCodes, ...local.failureCodes])],
    redactionCheck: rust.redactionCheck === "failed" || local.redactionCheck === "failed" ? "failed" : "passed",
    result,
  };
  return evaluateReport(validateReport(merged));
}
