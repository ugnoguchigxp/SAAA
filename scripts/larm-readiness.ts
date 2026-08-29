import { createHash, randomBytes } from "node:crypto";
import {
  constants as fsConstants,
  existsSync,
  fstatSync,
  lstatSync,
  openSync,
  readSync,
  readdirSync,
  realpathSync,
  statSync,
  unlinkSync,
  writeFileSync,
  closeSync,
  fsyncSync,
  chmodSync,
  linkSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { z } from "zod";
import { Database } from "bun:sqlite";
import { modelProvidersSettingsSchema, routingSettingsSchema, validateSettingsDocuments } from "../src/lib/schemas.ts";

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
const RESIDENT_DEFAULT_RUNTIME_IDS = new Set(["qwen-general"]);
const OPTIONAL_RUNTIME_IDS = new Set<string>();

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const RELEASE_BUNDLE = join(ROOT, "src-tauri/target/release/bundle/macos/SAAA.app");
const RELEASE_EXECUTABLE = join(RELEASE_BUNDLE, "Contents/MacOS/saaa");
const MAX_REPORT_BYTES = 64 * 1024;
const MAX_MANIFEST_BYTES = 16 * 1024;
const MAX_CHILD_BYTES = 1024 * 1024;
const MAX_BUILD_BYTES = 8 * 1024 * 1024;

const FAILURE_CODES = [
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

const SCENARIO_KEYS = [
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

const RESULT_KEYS = [
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

const TIMING_KEYS = [
  "elapsedMs",
  "sampleIntervalSeconds",
  "rssMaxSamplingGapSeconds",
  "metricsMaxSamplingGapSeconds",
  "plannedLarmRestartGapSeconds",
  "releaseRecoveryMaxMs",
  "ttlRecoveryMaxMs",
] as const;

const RESOURCE_KEYS = [
  "baselineActiveAllocations",
  "maxActiveAllocations",
  "finalActiveAllocations",
  "rssRangeMiB",
  "rssPrevious30mMedianMiB",
  "rssLast30mMedianMiB",
] as const;

const LEASE_KEYS = [
  "effectiveTtlSecondsMin",
  "effectiveTtlSecondsMax",
  "renewalsAttempted",
  "renewalsSucceeded",
] as const;

const count = z.number().int().min(0).max(10_000);
const milliseconds = z.number().int().min(0).max(10_800_000);
const seconds = z.number().int().min(0).max(10_800);
const rss = z.number().int().finite().min(0).max(1_048_576);
const commit = z.string().regex(/^[0-9a-f]{7,64}$/);
const sha256 = z.string().regex(/^[0-9a-f]{64}$/);
const revision = z.string().regex(/^[A-Za-z0-9._-]{1,64}$/);
const utcTimestamp = z.string().refine((value) => {
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

function utcTimestampSortKey(value: string): string {
  const [whole, fraction = ""] = value.slice(0, -1).split(".", 2);
  return `${whole}.${fraction.padEnd(9, "0")}`;
}

const scenarioSchema = z.object(Object.fromEntries(SCENARIO_KEYS.map((key) => [key, count])) as Record<typeof SCENARIO_KEYS[number], typeof count>).strict();
const resultCountSchema = z.object(Object.fromEntries(RESULT_KEYS.map((key) => [key, count])) as Record<typeof RESULT_KEYS[number], typeof count>).strict();
const timingSchema = z.object({
  elapsedMs: milliseconds,
  sampleIntervalSeconds: seconds,
  rssMaxSamplingGapSeconds: seconds,
  metricsMaxSamplingGapSeconds: seconds,
  plannedLarmRestartGapSeconds: seconds,
  releaseRecoveryMaxMs: milliseconds,
  ttlRecoveryMaxMs: milliseconds,
}).strict();
const resourceSchema = z.object({
  baselineActiveAllocations: count,
  maxActiveAllocations: count,
  finalActiveAllocations: count,
  rssRangeMiB: rss,
  rssPrevious30mMedianMiB: rss,
  rssLast30mMedianMiB: rss,
}).strict();
const leaseSchema = z.object({
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

const manifestSchema = z.object({
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

function usage(): never {
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

function zeroRecord<const T extends readonly string[]>(keys: T): Record<T[number], number> {
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

function reportFailure(report: ReadinessReport): RunnerError | undefined {
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

function modeBits(value: number): number {
  return value & 0o777;
}

function assertNoSymlinkComponents(target: string): void {
  const absolute = resolve(target);
  const root = absolute.startsWith(sep) ? sep : absolute.slice(0, absolute.indexOf(sep) + 1);
  const parts = absolute.slice(root.length).split(sep).filter(Boolean);
  let current = root;
  for (const part of parts) {
    current = join(current, part);
    if (!existsSync(current)) break;
    if (lstatSync(current).isSymbolicLink()) throw new RunnerError(3, "environment-invalid", "blocked");
  }
}

function assertCurrentOwner(target: string): void {
  if (typeof process.getuid === "function" && statSync(target).uid !== process.getuid()) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
}

function canonicalDirectory(target: string, expectedMode: number): string {
  try {
    if (!isAbsolute(target) || resolve(target) !== target) throw new RunnerError(3, "environment-invalid", "blocked");
    assertNoSymlinkComponents(target);
    const info = lstatSync(target);
    if (!info.isDirectory() || info.isSymbolicLink() || modeBits(info.mode) !== expectedMode) {
      throw new RunnerError(3, "environment-invalid", "blocked");
    }
    assertCurrentOwner(target);
    const canonical = realpathSync(target);
    if (canonical !== target) throw new RunnerError(3, "environment-invalid", "blocked");
    return canonical;
  } catch (error) {
    if (error instanceof RunnerError) throw error;
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
}

function isAncestorOrSame(left: string, right: string): boolean {
  const value = relative(left, right);
  return value === "" || (!value.startsWith(`..${sep}`) && value !== "..");
}

function assertSeparated(paths: string[]): void {
  for (let left = 0; left < paths.length; left += 1) {
    for (let right = left + 1; right < paths.length; right += 1) {
      if (isAncestorOrSame(paths[left]!, paths[right]!) || isAncestorOrSame(paths[right]!, paths[left]!)) {
        throw new RunnerError(3, "environment-invalid", "blocked");
      }
    }
  }
}

export function validateReportDirectory(target: string, command: CliArguments["command"], duration?: "30m" | "2h"): string {
  try {
    const canonical = canonicalDirectory(target, 0o700);
    const entries = readdirSync(canonical).sort();
    const expected = command === "preflight"
      ? []
      : command === "canary"
        ? [REPORT_FILENAMES.preflight]
        : command === "soak" && duration === "30m"
          ? [REPORT_FILENAMES.preflight, REPORT_FILENAMES.functional]
          : command === "soak" && duration === "2h"
            ? [REPORT_FILENAMES.preflight, REPORT_FILENAMES.functional, REPORT_FILENAMES["soak-30m"]]
            : [REPORT_FILENAMES.preflight, REPORT_FILENAMES.functional, REPORT_FILENAMES["soak-30m"], REPORT_FILENAMES["soak-2h"]];
    if (entries.length !== expected.length || entries.some((entry, index) => entry !== [...expected].sort()[index])) {
      throw new RunnerError(3, "environment-invalid", "blocked");
    }
    for (const entry of entries) {
      const filename = join(canonical, entry);
      const info = lstatSync(filename);
      if (!info.isFile() || info.isSymbolicLink() || info.nlink !== 1 || modeBits(info.mode) !== 0o600) {
        throw new RunnerError(3, "environment-invalid", "blocked");
      }
      assertCurrentOwner(filename);
    }
    return canonical;
  } catch (error) {
    if (error instanceof RunnerError) throw error;
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
}

interface StableFileIdentity {
  dev: number;
  ino: number;
  size: number;
  nlink: number;
  mode: number;
  mtimeMs: number;
  ctimeMs: number;
}

function stableFileIdentity(info: ReturnType<typeof fstatSync>): StableFileIdentity {
  return {
    dev: info.dev,
    ino: info.ino,
    size: info.size,
    nlink: info.nlink,
    mode: info.mode,
    mtimeMs: info.mtimeMs,
    ctimeMs: info.ctimeMs,
  };
}

function sameFileIdentity(left: StableFileIdentity, right: StableFileIdentity): boolean {
  return (Object.keys(left) as Array<keyof StableFileIdentity>).every((key) => left[key] === right[key]);
}

function sameFileObject(left: StableFileIdentity, right: StableFileIdentity): boolean {
  return left.dev === right.dev
    && left.ino === right.ino
    && left.nlink === right.nlink
    && left.mode === right.mode;
}

function readBoundedRegularFile(
  filename: string,
  maximumBytes: number,
  failure: () => RunnerError,
  expectedMode?: number,
): { bytes: Buffer; identity: StableFileIdentity } {
  let descriptor: number | undefined;
  try {
    assertNoSymlinkComponents(filename);
    descriptor = openSync(filename, fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW);
    const beforeInfo = fstatSync(descriptor);
    const before = stableFileIdentity(beforeInfo);
    if (
      !beforeInfo.isFile()
      || beforeInfo.isSymbolicLink()
      || before.nlink !== 1
      || before.size > maximumBytes
      || (expectedMode !== undefined && modeBits(before.mode) !== expectedMode)
      || (typeof process.getuid === "function" && beforeInfo.uid !== process.getuid())
    ) {
      throw failure();
    }
    const chunks: Buffer[] = [];
    let total = 0;
    while (true) {
      const chunk = Buffer.allocUnsafe(Math.min(64 * 1_024, maximumBytes + 1 - total));
      const count = readSync(descriptor, chunk, 0, chunk.length, null);
      if (count === 0) break;
      total += count;
      if (total > maximumBytes) throw failure();
      chunks.push(chunk.subarray(0, count));
    }
    const afterInfo = fstatSync(descriptor);
    const after = stableFileIdentity(afterInfo);
    if (!sameFileIdentity(before, after) || total !== before.size) throw failure();
    return { bytes: Buffer.concat(chunks, total), identity: before };
  } catch {
    throw failure();
  } finally {
    if (descriptor !== undefined) closeSync(descriptor);
  }
}

function parseJsonBytes(bytes: Buffer, failure: () => RunnerError): unknown {
  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    return JSON.parse(text);
  } catch {
    throw failure();
  }
}

function validateRollbackEndpoint(provider: CanaryManifest["rollbackProvider"]): void {
  let endpoint: URL;
  try {
    endpoint = new URL(provider.endpoint);
  } catch {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
  if (endpoint.username || endpoint.password) throw new RunnerError(3, "environment-invalid", "blocked");
  if (provider.location === "cloud") {
    if (endpoint.protocol !== "https:") throw new RunnerError(3, "environment-invalid", "blocked");
    return;
  }
  const hostname = endpoint.hostname.toLowerCase();
  const octets = hostname.split(".").map(Number);
  const privateIpv4 = octets.length === 4
    && octets.every((part) => Number.isInteger(part) && part >= 0 && part <= 255)
    && (octets[0] === 10
      || octets[0] === 127
      || (octets[0] === 172 && octets[1]! >= 16 && octets[1]! <= 31)
      || (octets[0] === 192 && octets[1] === 168));
  if (endpoint.protocol !== "http:" || !(hostname === "localhost" || hostname === "[::1]" || privateIpv4)) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
}

function loadManifestUnchecked(filename: string, phase: "preflight" | "functional" | "later", repositoryRoot = ROOT): { manifest: CanaryManifest; sha256: string; canonicalFile: string } {
  if (!isAbsolute(filename) || resolve(filename) !== filename) throw new RunnerError(3, "environment-invalid", "blocked");
  assertNoSymlinkComponents(filename);
  const info = lstatSync(filename);
  if (!info.isFile() || info.isSymbolicLink() || info.nlink !== 1 || info.size > MAX_MANIFEST_BYTES || modeBits(info.mode) !== 0o600) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
  assertCurrentOwner(filename);
  assertCurrentOwner(dirname(filename));
  const failure = () => new RunnerError(3, "environment-invalid", "blocked");
  const { bytes } = readBoundedRegularFile(filename, MAX_MANIFEST_BYTES, failure, 0o600);
  const parsed = manifestSchema.safeParse(parseJsonBytes(bytes, failure));
  if (!parsed.success) throw new RunnerError(3, "environment-invalid", "blocked");
  const manifest = parsed.data;
  validateNumericLoopbackOrigin(manifest.larmProvider.baseUrl);
  validateRollbackEndpoint(manifest.rollbackProvider);
  const canonicalFile = realpathSync(filename);
  if (canonicalFile !== filename) throw new RunnerError(3, "environment-invalid", "blocked");
  const canonicalData = canonicalDirectory(manifest.dataDirectory, 0o700);
  const home = realpathSync(homedir());
  const filesystemRoot = realpathSync(sep);
  const repository = realpathSync(repositoryRoot);
  const normalDataPath = join(home, "Library", "Application Support", "com.saaa.desktop");
  const normalData = existsSync(normalDataPath) ? realpathSync(normalDataPath) : resolve(normalDataPath);
  if (
    canonicalData === home
    || canonicalData === filesystemRoot
    || isAncestorOrSame(canonicalData, repository)
    || isAncestorOrSame(canonicalData, normalData)
    || isAncestorOrSame(normalData, canonicalData)
  ) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
  const dataEntries = readdirSync(canonicalData);
  if (phase === "preflight" || phase === "functional" ? dataEntries.length !== 0 : !dataEntries.includes("saaa.sqlite3")) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
  return { manifest: { ...manifest, dataDirectory: canonicalData }, sha256: createHash("sha256").update(bytes).digest("hex"), canonicalFile };
}

export function loadManifest(filename: string, phase: "preflight" | "functional" | "later", repositoryRoot = ROOT): { manifest: CanaryManifest; sha256: string; canonicalFile: string } {
  try {
    return loadManifestUnchecked(filename, phase, repositoryRoot);
  } catch (error) {
    if (error instanceof RunnerError) throw error;
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
}

function assertManifestSeparation(manifestFile: string, dataDirectory: string, reportDirectory: string): void {
  assertSeparated([realpathSync(ROOT), manifestFile, dataDirectory, reportDirectory]);
}

export interface ValidatedEnvironment {
  baseUrl: string;
  deployedCommit: string;
  deploymentRevision: string;
  token: string;
  rollbackCredential?: string;
  reportDirectory: string;
  dataDirectory: string;
  manifest: CanaryManifest;
  manifestSha256: string;
  manifestFile: string;
}

const CALLER_FORBIDDEN_VARIABLES = [
  "SAAA_SMOKE_MARKER_ID",
  "SAAA_SMOKE_EXERCISE_SITUATION",
  "SAAA_LARM_CANARY_RESULT_FILE",
  "SAAA_LARM_CANARY_ARTIFACT_SHA256",
  "SAAA_LARM_CANARY_MANIFEST_SHA256",
  "SAAA_LARM_CANARY_SAAA_COMMIT",
  "SAAA_LARM_CANARY_METRICS_SCOPE",
] as const;

function validateLiveEnvironmentUnchecked(
  reportDirectory: string,
  phase: "preflight" | "functional" | "later",
  environment: Record<string, string | undefined> = process.env,
): ValidatedEnvironment {
  if (FROZEN_LARM_CONTRACT_COMMIT === null) throw new RunnerError(3, "gate-missing", "blocked");
  if (environment.SAAA_LARM_CANARY !== "1" || environment.SAAA_LARM_ENABLED !== "1") {
    throw new RunnerError(3, "gate-missing", "blocked");
  }
  if (CALLER_FORBIDDEN_VARIABLES.some((key) => environment[key] !== undefined)) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
  const token = validateHeaderCredential(environment.LARM_API_TOKEN);
  const baseUrl = validateNumericLoopbackOrigin(environment.SAAA_LARM_CANARY_BASE_URL ?? "");
  const deployedCommit = commit.safeParse(environment.SAAA_LARM_DEPLOYED_COMMIT);
  const deploymentRevision = revision.safeParse(environment.SAAA_LARM_DEPLOYMENT_REVISION);
  const manifestFilename = environment.SAAA_LARM_CANARY_MANIFEST_FILE;
  const environmentReportDirectory = environment.SAAA_LARM_CANARY_REPORT_DIR;
  const environmentDataDirectory = environment.SAAA_SMOKE_DATA_DIR;
  if (!deployedCommit.success || !deploymentRevision.success || manifestFilename === undefined || environmentReportDirectory === undefined || environmentDataDirectory === undefined) {
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
  const canonicalReport = realpathSync(reportDirectory);
  if (realpathSync(environmentReportDirectory) !== canonicalReport) throw new RunnerError(3, "environment-invalid", "blocked");
  const loaded = loadManifest(manifestFilename, phase);
  if (
    loaded.manifest.saaaCommit !== gitCommitSync()
    || loaded.manifest.larmContractCommit !== FROZEN_LARM_CONTRACT_COMMIT
    || loaded.manifest.larmContractCommit !== COMPILED_LARM_CONTRACT_COMMIT
    || loaded.manifest.larmContractCommit !== deployedCommit.data
    || loaded.manifest.deploymentRevision !== deploymentRevision.data
    || loaded.manifest.larmProvider.baseUrl !== baseUrl
    || loaded.manifest.dataDirectory !== realpathSync(environmentDataDirectory)
  ) {
    throw new RunnerError(2, "contract-mismatch", "failed");
  }
  assertManifestSeparation(loaded.canonicalFile, loaded.manifest.dataDirectory, canonicalReport);
  const rawRollbackCredential = environment[loaded.manifest.rollbackProvider.credentialEnv];
  if (loaded.manifest.rollbackProvider.credentialRequired && rawRollbackCredential === undefined) {
    throw new RunnerError(3, "gate-missing", "blocked");
  }
  const rollbackCredential = rawRollbackCredential === undefined
    ? undefined
    : validateHeaderCredential(rawRollbackCredential);
  return {
    baseUrl,
    deployedCommit: deployedCommit.data,
    deploymentRevision: deploymentRevision.data,
    token,
    ...(rollbackCredential === undefined ? {} : { rollbackCredential }),
    reportDirectory: canonicalReport,
    dataDirectory: loaded.manifest.dataDirectory,
    manifest: loaded.manifest,
    manifestSha256: loaded.sha256,
    manifestFile: loaded.canonicalFile,
  };
}

export function validateLiveEnvironment(
  reportDirectory: string,
  phase: "preflight" | "functional" | "later",
  environment: Record<string, string | undefined> = process.env,
): ValidatedEnvironment {
  try {
    return validateLiveEnvironmentUnchecked(reportDirectory, phase, environment);
  } catch (error) {
    if (error instanceof RunnerError) throw error;
    throw new RunnerError(3, "environment-invalid", "blocked");
  }
}

function gitCommitSync(): string {
  const result = Bun.spawnSync(["git", "rev-parse", "HEAD"], { cwd: ROOT, stdout: "pipe", stderr: "pipe", env: buildEnvironment(process.env) });
  const value = result.stdout.toString().trim();
  if (result.exitCode !== 0 || !commit.safeParse(value).success) throw new RunnerError(70, "internal", "failed");
  return value;
}

function assertCleanRepository(): string {
  const before = gitCommitSync();
  const result = Bun.spawnSync(["git", "status", "--porcelain=v1", "--untracked-files=all"], { cwd: ROOT, stdout: "pipe", stderr: "pipe", env: buildEnvironment(process.env) });
  if (result.exitCode !== 0) throw new RunnerError(70, "internal", "failed");
  if (result.stdout.byteLength !== 0) throw new RunnerError(3, "environment-invalid", "blocked");
  return before;
}

const BUILD_ENVIRONMENT_KEYS = [
  "PATH", "HOME", "TMPDIR", "TEMP", "TMP", "USER", "LOGNAME", "LANG", "LC_ALL",
  "CARGO_HOME", "RUSTUP_HOME", "SSL_CERT_FILE", "SSL_CERT_DIR", "NIX_SSL_CERT_FILE",
  "DEVELOPER_DIR", "SDKROOT", "MACOSX_DEPLOYMENT_TARGET",
] as const;
export function buildEnvironment(environment: Record<string, string | undefined>): Record<string, string> {
  const result: Record<string, string> = {};
  for (const key of BUILD_ENVIRONMENT_KEYS) if (environment[key] !== undefined) result[key] = environment[key]!;
  return result;
}

const RUST_BASE_ENVIRONMENT = ["PATH", "HOME", "TMPDIR", "TEMP", "TMP", "CARGO_HOME", "RUSTUP_HOME", "SSL_CERT_FILE", "SSL_CERT_DIR", "NIX_SSL_CERT_FILE"] as const;
export function rustChildEnvironment(environment: Record<string, string | undefined>, internal: {
  resultFile: string;
  artifactSha256: string;
  manifestSha256: string;
  metricsScope: CanaryManifest["metricsScope"];
  saaaCommit?: string;
}): Record<string, string> {
  const result: Record<string, string> = {};
  for (const key of RUST_BASE_ENVIRONMENT) if (environment[key] !== undefined) result[key] = environment[key]!;
  for (const key of ["SAAA_LARM_CANARY", "SAAA_LARM_ENABLED", "SAAA_LARM_CANARY_BASE_URL", "SAAA_LARM_DEPLOYED_COMMIT", "SAAA_LARM_DEPLOYMENT_REVISION", "LARM_API_TOKEN"] as const) {
    if (environment[key] !== undefined) result[key] = environment[key]!;
  }
  result.SAAA_LARM_CANARY_RESULT_FILE = internal.resultFile;
  result.SAAA_LARM_CANARY_ARTIFACT_SHA256 = internal.artifactSha256;
  result.SAAA_LARM_CANARY_MANIFEST_SHA256 = internal.manifestSha256;
  result.SAAA_LARM_CANARY_METRICS_SCOPE = internal.metricsScope;
  if (internal.saaaCommit !== undefined) result.SAAA_LARM_CANARY_SAAA_COMMIT = internal.saaaCommit;
  return result;
}

export function appChildEnvironment(environment: Record<string, string | undefined>, internal: {
  enabled: boolean;
  markerId: string;
  dataDirectory: string;
}): Record<string, string> {
  const result: Record<string, string> = {};
  for (const key of ["PATH", "HOME", "TMPDIR", "TEMP", "TMP", "USER", "LOGNAME", "LANG", "LC_ALL", "DISPLAY", "XDG_RUNTIME_DIR"] as const) {
    if (environment[key] !== undefined) result[key] = environment[key]!;
  }
  for (const key of ["SAAA_LARM_CANARY", "LARM_API_TOKEN", "SAAA_PROVIDER_LOCAL_OPENAI_COMPATIBLE_API_KEY"] as const) {
    if (environment[key] !== undefined) result[key] = environment[key]!;
  }
  result.SAAA_LARM_ENABLED = internal.enabled ? "1" : "0";
  result.SAAA_SMOKE_MARKER_ID = internal.markerId;
  result.SAAA_SMOKE_DATA_DIR = internal.dataDirectory;
  return result;
}

export class ForbiddenDataScanner {
  private overlap = Buffer.alloc(0);
  private failed = false;
  private readonly exact: Buffer[];

  constructor(values: Array<string | undefined>) {
    this.exact = values
      .filter((value): value is string => value !== undefined && Buffer.byteLength(value) > 0)
      .map((value) => Buffer.from(value));
  }

  scan(chunk: Uint8Array): boolean {
    const bytes = Buffer.concat([this.overlap, Buffer.from(chunk)]);
    const text = bytes.toString("utf8");
    this.failed ||= this.exact.some((needle) => bytes.includes(needle))
      || /authorization\s*:/i.test(text)
      || /bearer\s+[\x21-\x7e]+/i.test(text)
      || /(?:allocation|operation|request|conversation|runtime[_ -]?run)[_ -]?id["']?\s*[=:]/i.test(text);
    this.overlap = bytes.subarray(Math.max(0, bytes.length - 4_095));
    return this.failed;
  }

  get detected(): boolean {
    return this.failed;
  }

  fork(): ForbiddenDataScanner {
    return new ForbiddenDataScanner(this.exact.map((value) => value.toString("utf8")));
  }
}

interface ChildResult {
  exitCode: number;
  stdoutBytes: number;
  stderrBytes: number;
  redactionFailed: boolean;
}

async function consumeBounded(
  stream: ReadableStream<Uint8Array>,
  limit: number,
  scanner: ForbiddenDataScanner,
  onOverflow: () => void,
  onForbidden: () => void,
  onChunk?: (chunk: Uint8Array) => void,
): Promise<number> {
  const reader = stream.getReader();
  let size = 0;
  try {
    while (true) {
      const next = await reader.read();
      if (next.done) break;
      size += next.value.byteLength;
      if (size > limit) {
        onOverflow();
        throw new RunnerError(70, "internal", "failed");
      }
      onChunk?.(next.value);
      if (scanner.scan(next.value)) {
        onForbidden();
        throw new RunnerError(2, "redaction-failed", "failed");
      }
    }
  } finally {
    reader.releaseLock();
  }
  return size;
}

async function terminateOwnedChild(child: ReturnType<typeof Bun.spawn>): Promise<void> {
  const signalGroup = (signal: NodeJS.Signals): boolean => {
    if (child.pid > 1) {
      try {
        process.kill(-child.pid, signal);
        return true;
      } catch {
        // A non-detached helper has no child-owned process group; signal its exact PID below.
      }
    }
    if (child.exitCode === null) {
      try {
        child.kill(signal);
        return true;
      } catch {
        // The child crossed the exit boundary after the status check.
      }
    }
    return false;
  };
  const groupExists = (): boolean => {
    if (child.pid <= 1) return false;
    try {
      process.kill(-child.pid, 0);
      return true;
    } catch {
      return false;
    }
  };
  if (!signalGroup("SIGTERM")) return;
  const gracefulDeadline = performance.now() + 5_000;
  while (performance.now() < gracefulDeadline && (child.exitCode === null || groupExists())) {
    await Bun.sleep(50);
  }
  if (child.exitCode === null || groupExists()) signalGroup("SIGKILL");
  if (child.exitCode === null) {
    await child.exited;
  }
}

async function runBoundedChild(options: {
  command: string[];
  environment: Record<string, string>;
  limit: number;
  deadlineMs: number;
  scanner: ForbiddenDataScanner;
  signal?: AbortSignal;
}): Promise<ChildResult> {
  const child = Bun.spawn(options.command, {
    cwd: ROOT,
    env: options.environment,
    stdout: "pipe",
    stderr: "pipe",
    detached: true,
  });
  const abort = () => void terminateOwnedChild(child);
  if (options.signal?.aborted) abort();
  options.signal?.addEventListener("abort", abort, { once: true });
  let overflow = false;
  let redactionFailed = false;
  const stdoutScanner = options.scanner.fork();
  const stderrScanner = options.scanner.fork();
  const stdout = consumeBounded(child.stdout, options.limit, stdoutScanner, () => {
    overflow = true;
    void terminateOwnedChild(child);
  }, () => {
    redactionFailed = true;
    void terminateOwnedChild(child);
  });
  const stderr = consumeBounded(child.stderr, options.limit, stderrScanner, () => {
    overflow = true;
    void terminateOwnedChild(child);
  }, () => {
    redactionFailed = true;
    void terminateOwnedChild(child);
  });
  let deadlineHandle: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<never>((_, reject) => {
    deadlineHandle = setTimeout(() => {
      void terminateOwnedChild(child).finally(() => reject(new RunnerError(2, "runner-timeout", "failed")));
    }, options.deadlineMs);
  });
  let exitCode: number;
  try {
    exitCode = await Promise.race([child.exited, deadline]);
  } catch (error) {
    await Promise.allSettled([stdout, stderr]);
    throw error;
  } finally {
    if (deadlineHandle !== undefined) clearTimeout(deadlineHandle);
    options.signal?.removeEventListener("abort", abort);
  }
  await terminateOwnedChild(child);
  const streams = await Promise.allSettled([stdout, stderr]);
  if (redactionFailed) throw new RunnerError(2, "redaction-failed", "failed");
  if (overflow) throw new RunnerError(70, "internal", "failed");
  if (streams.some((stream) => stream.status === "rejected")) throw new RunnerError(70, "internal", "failed");
  const stdoutBytes = streams[0].status === "fulfilled" ? streams[0].value : 0;
  const stderrBytes = streams[1].status === "fulfilled" ? streams[1].value : 0;
  return { exitCode, stdoutBytes, stderrBytes, redactionFailed: stdoutScanner.detected || stderrScanner.detected };
}

interface OwnedApplication {
  child: ReturnType<typeof Bun.spawn>;
  stdoutScanner: ForbiddenDataScanner;
  stderrScanner: ForbiddenDataScanner;
  stdout: Promise<{ ok: boolean }>;
  stderr: Promise<{ ok: boolean }>;
  stdoutCapture: Buffer[];
  stderrCapture: Buffer[];
}

function applicationDetectedForbiddenData(application: OwnedApplication): boolean {
  return application.stdoutScanner.detected || application.stderrScanner.detected;
}

function capturedApplicationStream(
  stream: ReadableStream<Uint8Array>,
  scanner: ForbiddenDataScanner,
  child: ReturnType<typeof Bun.spawn>,
  capture: Buffer[],
): Promise<{ ok: boolean }> {
  return consumeBounded(
    stream,
    MAX_CHILD_BYTES,
    scanner,
    () => void terminateOwnedChild(child),
    () => void terminateOwnedChild(child),
    (chunk) => capture.push(Buffer.from(chunk)),
  ).then(
    () => ({ ok: true }),
    () => ({ ok: false }),
  );
}

async function startApplication(environment: ValidatedEnvironment, enabled: boolean): Promise<OwnedApplication> {
  const markerId = `canary-${randomBytes(16).toString("hex")}`;
  const marker = join(tmpdir(), `saaa-frontend-${markerId}.ready`);
  if (existsSync(marker)) throw new RunnerError(2, "environment-invalid", "failed");
  const scanner = new ForbiddenDataScanner([
    environment.token,
    environment.rollbackCredential,
    environment.baseUrl,
    environment.manifest.rollbackProvider.endpoint,
    "Reply with exactly: CANARY_OK",
    "List the numbers 1 through 5.",
    "Write one short greeting in Japanese.",
    "Write ten numbered words, one at a time.",
    "Reply with exactly: READY",
  ]);
  const child = Bun.spawn([RELEASE_EXECUTABLE], {
    cwd: ROOT,
    env: appChildEnvironment(process.env, { enabled, markerId, dataDirectory: environment.dataDirectory }),
    stdout: "pipe",
    stderr: "pipe",
    detached: true,
  });
  const stdoutScanner = scanner.fork();
  const stderrScanner = scanner.fork();
  const stdoutCapture: Buffer[] = [];
  const stderrCapture: Buffer[] = [];
  const application: OwnedApplication = {
    child,
    stdoutScanner,
    stderrScanner,
    stdout: capturedApplicationStream(child.stdout, stdoutScanner, child, stdoutCapture),
    stderr: capturedApplicationStream(child.stderr, stderrScanner, child, stderrCapture),
    stdoutCapture,
    stderrCapture,
  };
  const started = performance.now();
  while (performance.now() - started <= 10_000) {
    if (existsSync(marker)) {
      const info = lstatSync(marker);
      if (!info.isFile() || info.isSymbolicLink() || info.nlink !== 1) {
        await stopApplication(application);
        throw new RunnerError(2, "restart-recovery-failed", "failed");
      }
      unlinkSync(marker);
      return application;
    }
    if (child.exitCode !== null || applicationDetectedForbiddenData(application)) break;
    await Bun.sleep(100);
  }
  if (existsSync(marker)) unlinkSync(marker);
  await stopApplication(application);
  throw new RunnerError(2, applicationDetectedForbiddenData(application) ? "redaction-failed" : "restart-recovery-failed", "failed");
}

async function stopApplication(application: OwnedApplication, knownIdentifiers: string[] = []): Promise<void> {
  await terminateOwnedChild(application.child);
  const [stdout, stderr] = await Promise.all([application.stdout, application.stderr]);
  if (!stdout.ok || !stderr.ok || applicationDetectedForbiddenData(application)) {
    throw new RunnerError(2, applicationDetectedForbiddenData(application) ? "redaction-failed" : "internal", "failed");
  }
  const stdoutIdentifierScanner = new ForbiddenDataScanner(knownIdentifiers);
  const stderrIdentifierScanner = new ForbiddenDataScanner(knownIdentifiers);
  for (const chunk of application.stdoutCapture) stdoutIdentifierScanner.scan(chunk);
  for (const chunk of application.stderrCapture) stderrIdentifierScanner.scan(chunk);
  application.stdoutCapture.length = 0;
  application.stderrCapture.length = 0;
  if (stdoutIdentifierScanner.detected || stderrIdentifierScanner.detected) {
    throw new RunnerError(2, "redaction-failed", "failed");
  }
}

interface SettingsState {
  larmId: string;
  directPrimary: boolean;
  larmPrimary: boolean;
}

const databaseIdentifier = z.string().min(1).max(160).regex(/^[A-Za-z0-9_-]+$/);
const databaseProviderId = z.string().min(1).max(80).regex(/^[A-Za-z0-9_-]+$/);
const runtimeStatus = z.enum(["running", "completed", "failed", "cancelled", "interrupted"]);
const runtimeRowSchema = z.object({
  id: databaseIdentifier,
  conversation_id: databaseIdentifier,
  provider_id: databaseProviderId.nullable(),
  status: runtimeStatus,
}).strict();
const providerSessionRowSchema = z.object({
  id: databaseIdentifier,
  runtime_run_id: databaseIdentifier.nullable(),
  provider_id: databaseProviderId,
  provider_kind: z.enum(["openai-compatible", "larm"]).nullable(),
  allocation_id: databaseIdentifier.nullable(),
  selected_runtime_id: databaseIdentifier.nullable(),
  request_id: databaseIdentifier.nullable(),
  fallback_used: z.union([z.literal(0), z.literal(1)]).nullable(),
  route_id: z.string().min(1).max(80).regex(/^[A-Za-z0-9._-]+$/).nullable(),
  selection_reason: z.enum(["primary", "other"]).nullable(),
  output_started: z.union([z.literal(0), z.literal(1)]).nullable(),
  failure_kind: z.enum([
    "authentication", "contract", "protocol", "request-too-large", "internal", "client-disconnected",
    "cancelled", "partial-output", "policy", "capacity", "unavailable", "draining", "upstream", "network",
    "timeout", "allocation-lost", "allocation-outcome-unknown", "not-ready",
  ]).nullable(),
  release_status: z.enum(["not-applicable", "not-started", "pending", "released", "failed", "deferred-to-ttl"]),
  status: runtimeStatus,
}).strict();

type RuntimeRow = z.infer<typeof runtimeRowSchema>;
type ProviderSessionRow = z.infer<typeof providerSessionRowSchema>;

interface DatabaseSnapshot {
  runtimeIds: Set<string>;
  sessionIds: Set<string>;
}

interface DatabaseObservation {
  runs: RuntimeRow[];
  sessions: ProviderSessionRow[];
}

function openCanaryDatabase(dataDirectory: string): Database {
  const filename = join(dataDirectory, "saaa.sqlite3");
  let database: Database | undefined;
  try {
    assertNoSymlinkComponents(filename);
    for (const suffix of ["-wal", "-shm", "-journal"]) {
      const sidecar = `${filename}${suffix}`;
      if (!existsSync(sidecar)) continue;
      assertNoSymlinkComponents(sidecar);
      const sidecarInfo = lstatSync(sidecar);
      if (!sidecarInfo.isFile() || sidecarInfo.isSymbolicLink() || sidecarInfo.nlink !== 1) {
        throw new Error("invalid database sidecar");
      }
      assertCurrentOwner(sidecar);
    }
    const info = lstatSync(filename);
    if (!info.isFile() || info.isSymbolicLink() || info.nlink !== 1) throw new Error("invalid database");
    const initialIdentity = stableFileIdentity(info);
    assertCurrentOwner(filename);
    database = new Database(filename, { create: false, readonly: true });
    const openedInfo = lstatSync(filename);
    if (!sameFileObject(initialIdentity, stableFileIdentity(openedInfo))) throw new Error("database changed while opening");
    database.exec("PRAGMA query_only=ON; PRAGMA busy_timeout=1000;");
    const required: Record<string, string[]> = {
      settings_documents: ["namespace", "key", "schema_version", "value_json"],
      runtime_runs: ["id", "conversation_id", "provider_id", "status"],
      provider_sessions: [
        "id", "runtime_run_id", "provider_id", "provider_kind", "allocation_id",
        "selected_runtime_id", "request_id", "fallback_used", "route_id", "selection_reason", "output_started", "failure_kind",
        "release_status", "status",
      ],
    };
    for (const [table, columns] of Object.entries(required)) {
      const entry = database.query("SELECT type FROM sqlite_master WHERE name=?1").get(table) as { type: string } | null;
      if (entry?.type !== "table") throw new Error("missing table");
      const rows = database.query(`PRAGMA table_info('${table}')`).all() as Array<{ name: string }>;
      const names = new Set(rows.map((row) => row.name));
      if (columns.some((column) => !names.has(column))) throw new Error("missing schema");
    }
    return database;
  } catch {
    database?.close();
    throw new RunnerError(2, "database-schema-invalid", "failed");
  }
}

function settingsState(database: Database, manifest: CanaryManifest): SettingsState {
  let transactionOpen = false;
  try {
    database.exec("BEGIN");
    transactionOpen = true;
    const bound = database.query(
      `SELECT COUNT(*) AS count,
              COALESCE(MAX(length(CAST(value_json AS BLOB))),0) AS maximum,
              COALESCE(SUM(length(CAST(value_json AS BLOB))),0) AS total
       FROM settings_documents`,
    ).get() as { count: number; maximum: number; total: number } | null;
    if (
      bound === null
      || !Number.isSafeInteger(bound.count)
      || !Number.isSafeInteger(bound.maximum)
      || !Number.isSafeInteger(bound.total)
      || bound.count > 1_000
      || bound.maximum > 256 * 1_024
      || bound.total > 1024 * 1_024
    ) {
      throw new Error("settings bounds exceeded");
    }
    const rows = database.query(
      "SELECT namespace,key,schema_version,value_json FROM settings_documents ORDER BY namespace,key",
    ).all() as Array<{ namespace: string; key: string; schema_version: number; value_json: string }>;
    const documents = rows.map((row) => ({
      namespace: row.namespace,
      key: row.key,
      schemaVersion: row.schema_version,
      valueJson: JSON.parse(row.value_json) as unknown,
    }));
    validateSettingsDocuments(documents);
    const providersDocument = documents.find((document) => document.namespace === "providers.model");
    const routingDocument = documents.find((document) => document.namespace === "routing.tasks");
    if (providersDocument === undefined || routingDocument === undefined) throw new Error("missing settings");
    const providers = modelProvidersSettingsSchema.parse(providersDocument.valueJson).providers;
    const routing = routingSettingsSchema.parse(routingDocument.valueJson);
    if (providers.length !== 2) throw new Error("unexpected providers");
    const larm = providers.find((provider) => provider.kind === "larm");
    const direct = providers.find((provider) => provider.kind === "openai-compatible" && provider.id === "local-openai-compatible");
    if (
      larm === undefined
      || direct === undefined
      || !larm.enabled
      || !direct.enabled
      || larm.baseUrl !== manifest.larmProvider.baseUrl
      || larm.allocationTtlSeconds !== manifest.larmProvider.allocationTtlSeconds
      || larm.allocationStartupTimeoutSeconds !== manifest.larmProvider.allocationStartupTimeoutSeconds
      || larm.allowFallbackByDefault !== false
      || larm.deploymentPolicy !== "existing-only"
      || direct.location !== manifest.rollbackProvider.location
      || direct.endpoint !== manifest.rollbackProvider.endpoint
      || direct.model !== manifest.rollbackProvider.model
    ) {
      throw new Error("settings mismatch");
    }
    const route = routing.conversationRespond;
    const state = {
      larmId: larm.id,
      directPrimary: route.primaryProviderId === direct.id && route.fallbackProviderIds.length === 0,
      larmPrimary: route.primaryProviderId === larm.id
        && route.fallbackProviderIds.length === 1
        && route.fallbackProviderIds[0] === direct.id,
    };
    database.exec("COMMIT");
    transactionOpen = false;
    return state;
  } catch {
    if (transactionOpen) {
      try {
        database.exec("ROLLBACK");
      } catch {
        // The connection is closed by the caller; keep the strict database error.
      }
    }
    throw new RunnerError(2, "database-schema-invalid", "failed");
  }
}

function databaseSnapshot(database: Database): DatabaseSnapshot {
  let transactionOpen = false;
  try {
    database.exec("BEGIN");
    transactionOpen = true;
    const runtimeRows = database.query("SELECT id FROM runtime_runs LIMIT 10001").all() as Array<{ id: string }>;
    const sessionRows = database.query("SELECT id FROM provider_sessions LIMIT 10001").all() as Array<{ id: string }>;
    if (runtimeRows.length > 10_000 || sessionRows.length > 10_000) throw new Error("snapshot too large");
    const parsedRuntimeRows = z.array(z.object({ id: databaseIdentifier }).strict()).max(10_000).parse(runtimeRows);
    const parsedSessionRows = z.array(z.object({ id: databaseIdentifier }).strict()).max(10_000).parse(sessionRows);
    const snapshot = {
      runtimeIds: new Set(parsedRuntimeRows.map((row) => row.id)),
      sessionIds: new Set(parsedSessionRows.map((row) => row.id)),
    };
    database.exec("COMMIT");
    transactionOpen = false;
    return snapshot;
  } catch {
    if (transactionOpen) {
      try {
        database.exec("ROLLBACK");
      } catch {
        // The connection is closed by the caller; do not replace the bounded schema error.
      }
    }
    throw new RunnerError(2, "database-schema-invalid", "failed");
  }
}

function databaseObservation(database: Database, snapshot: DatabaseSnapshot): DatabaseObservation {
  let transactionOpen = false;
  try {
    database.exec("BEGIN");
    transactionOpen = true;
    const rawRuns = database.query(
      "SELECT id,conversation_id,provider_id,status FROM runtime_runs LIMIT 10001",
    ).all();
    const rawSessions = database.query(
      `SELECT id,runtime_run_id,provider_id,provider_kind,allocation_id,selected_runtime_id,request_id,
              fallback_used,route_id,selection_reason,output_started,failure_kind,release_status,status
       FROM provider_sessions LIMIT 10001`,
    ).all();
    if (rawRuns.length > 10_000 || rawSessions.length > 10_000) throw new Error("observation too large");
    const allRuns = z.array(runtimeRowSchema).max(10_000).parse(rawRuns);
    const allSessions = z.array(providerSessionRowSchema).max(10_000).parse(rawSessions);
    const observation = {
      runs: allRuns.filter((row) => !snapshot.runtimeIds.has(row.id)),
      sessions: allSessions.filter((row) => !snapshot.sessionIds.has(row.id)),
    };
    database.exec("COMMIT");
    transactionOpen = false;
    return observation;
  } catch {
    if (transactionOpen) {
      try {
        database.exec("ROLLBACK");
      } catch {
        // The connection is closed by the caller; do not replace the bounded schema error.
      }
    }
    throw new RunnerError(2, "database-schema-invalid", "failed");
  }
}

async function waitForCheckpoint(
  application: OwnedApplication,
  code: string,
  predicate: () => boolean,
  deadlineAt: number,
): Promise<void> {
  fixedProgress(code);
  const checkpointDeadline = Math.min(deadlineAt, performance.now() + 5 * 60_000);
  while (performance.now() < checkpointDeadline) {
    if (application.child.exitCode !== null || applicationDetectedForbiddenData(application)) {
      throw new RunnerError(2, applicationDetectedForbiddenData(application) ? "redaction-failed" : "restart-recovery-failed", "failed");
    }
    if (predicate()) return;
    await Bun.sleep(1_000);
  }
  throw new RunnerError(3, "gate-missing", "blocked");
}

function withCanaryDatabase<T>(dataDirectory: string, operation: (database: Database) => T): T {
  const database = openCanaryDatabase(dataDirectory);
  try {
    return operation(database);
  } finally {
    database.close();
  }
}

function observeDatabase(environment: ValidatedEnvironment, snapshot: DatabaseSnapshot): DatabaseObservation {
  return withCanaryDatabase(environment.dataDirectory, (database) => databaseObservation(database, snapshot));
}

function observeSettings(environment: ValidatedEnvironment): SettingsState {
  return withCanaryDatabase(environment.dataDirectory, (database) => settingsState(database, environment.manifest));
}

function assertLarmPrimarySettings(environment: ValidatedEnvironment, larmId: string): void {
  const settings = observeSettings(environment);
  if (!settings.larmPrimary || settings.larmId !== larmId) {
    throw new RunnerError(2, "rollback-failed", "failed");
  }
}

function countSessions(
  observation: DatabaseObservation,
  predicate: (session: ProviderSessionRow) => boolean,
): number {
  return observation.sessions.filter(predicate).length;
}

function knownObservationIdentifiers(observation: DatabaseObservation): string[] {
  return [
    ...observation.runs.flatMap((run) => [run.id, run.conversation_id]),
    ...observation.sessions.flatMap((session) => [
      session.id,
      session.runtime_run_id,
      session.allocation_id,
      session.selected_runtime_id,
      session.request_id,
    ].filter((value): value is string => value !== null)),
  ];
}

function knownDatabaseIdentifiersOrEmpty(
  environment: ValidatedEnvironment,
  snapshot: DatabaseSnapshot,
): string[] {
  try {
    return knownObservationIdentifiers(observeDatabase(environment, snapshot));
  } catch {
    return [];
  }
}

function runtimeCategory(runtimeId: string): "resident-default" | "optional" | "unknown" {
  if (RESIDENT_DEFAULT_RUNTIME_IDS.has(runtimeId)) return "resident-default";
  return OPTIONAL_RUNTIME_IDS.has(runtimeId) ? "optional" : "unknown";
}

function validateFunctionalObservation(
  observation: DatabaseObservation,
  larmId: string,
): void {
  const larm = observation.sessions.filter((session) => session.provider_id === larmId && session.provider_kind === "larm");
  const direct = observation.sessions.filter((session) => session.provider_id === "local-openai-compatible" && session.provider_kind === "openai-compatible");
  const failureKinds = larm.filter((session) => session.status === "failed").map((session) => session.failure_kind).sort();
  const failureByKind = (kind: string) => larm.find((session) => session.status === "failed" && session.failure_kind === kind);
  const statusCounts = (status: string) => observation.runs.filter((run) => run.status === status).length;
  const allocationIds = larm.flatMap((session) => session.allocation_id === null ? [] : [session.allocation_id]);
  const selectedRuntimes = larm.flatMap((session) => session.selected_runtime_id === null ? [] : [session.selected_runtime_id]);
  const runIds = new Set(observation.runs.map((run) => run.id));
  const releaseInvalid = larm.some((session) => session.allocation_id !== null && !["released", "deferred-to-ttl"].includes(session.release_status));
  const implicitFallback = larm.some((session) => session.fallback_used !== 0);
  const runtimePolicyInvalid = larm.some((session) => {
    if (session.selected_runtime_id === null) {
      return session.failure_kind !== "capacity"
        || session.allocation_id !== null
        || session.route_id !== null
        || session.selection_reason !== null
        || session.release_status !== "not-started";
    }
    return session.allocation_id === null
      || session.route_id !== "llm-default"
      || session.selection_reason !== "primary";
  });
  if (
    implicitFallback
    || runtimePolicyInvalid
    || selectedRuntimes.some((runtime) => runtimeCategory(runtime) !== "resident-default")
  ) {
    throw new RunnerError(2, "runtime-policy-violation", "failed");
  }
  if (new Set(allocationIds).size !== allocationIds.length) {
    throw new RunnerError(2, "runtime-policy-violation", "failed");
  }
  if (releaseInvalid) throw new RunnerError(2, "allocation-leak", "failed");
  if (
    observation.runs.length !== 14
    || observation.sessions.length !== 16
    || observation.sessions.some((session) => session.runtime_run_id === null || !runIds.has(session.runtime_run_id))
    || observation.runs.some((run) => !observation.sessions.some((session) => session.runtime_run_id === run.id))
    || statusCounts("completed") !== 12
    || statusCounts("cancelled") !== 1
    || statusCounts("failed") !== 1
    || larm.length !== 7
    || direct.length !== 9
    || larm.filter((session) => session.status === "completed").length !== 3
    || larm.filter((session) => session.status === "cancelled").length !== 1
    || direct.some((session) => session.status !== "completed"
      || session.failure_kind !== null
      || session.fallback_used !== 0
      || session.output_started !== 1
      || session.allocation_id !== null
      || session.selected_runtime_id !== null
      || session.request_id !== null
      || session.route_id !== null
      || session.selection_reason !== null
      || session.release_status !== "not-applicable")
    || failureKinds.join(",") !== "capacity,partial-output,timeout"
    || failureByKind("partial-output")?.output_started !== 1
    || failureByKind("timeout")?.output_started !== 0
    || failureByKind("capacity")?.output_started !== 0
    || larm.some((session) => session.status === "completed"
      ? session.failure_kind !== null || session.output_started !== 1
      : session.status === "cancelled"
        ? session.failure_kind !== "cancelled" || session.output_started !== 1
        : false)
    || observation.runs.some((run) => {
      const sessions = observation.sessions.filter((session) => session.runtime_run_id === run.id);
      if (run.status === "completed") {
        return !(
          (sessions.length === 1 && sessions[0]!.status === "completed")
          || (sessions.length === 2
            && sessions.some((session) => session.provider_id === larmId
              && session.status === "failed"
              && ["capacity", "timeout"].includes(session.failure_kind ?? ""))
            && sessions.some((session) => session.provider_id === "local-openai-compatible" && session.status === "completed"))
        );
      }
      if (run.status === "cancelled") {
        return sessions.length !== 1
          || sessions[0]!.provider_id !== larmId
          || sessions[0]!.status !== "cancelled";
      }
      if (run.status === "failed") {
        return sessions.length !== 1
          || sessions[0]!.provider_id !== larmId
          || sessions[0]!.status !== "failed"
          || sessions[0]!.failure_kind !== "partial-output";
      }
      return true;
    })
  ) {
    throw new RunnerError(2, "database-schema-invalid", "failed");
  }
  const explicitFallbacks = observation.runs.filter((run) => {
    const sessions = observation.sessions.filter((session) => session.runtime_run_id === run.id);
    return sessions.some((session) => session.provider_id === larmId && ["capacity", "timeout"].includes(session.failure_kind ?? ""))
      && sessions.some((session) => session.provider_id === "local-openai-compatible" && session.status === "completed");
  }).length;
  if (explicitFallbacks !== 2) throw new RunnerError(2, "rollback-failed", "failed");
}

async function observeFunctional(
  environment: ValidatedEnvironment,
  identity: { artifactSha256: string; saaaCommit: string },
  deadlineAt: number,
): Promise<{ local: ReadinessReport; rust: ReadinessReport }> {
  let application: OwnedApplication | undefined = await startApplication(environment, false);
  let snapshot: DatabaseSnapshot;
  let primaryError: unknown;
  try {
    snapshot = withCanaryDatabase(environment.dataDirectory, databaseSnapshot);
    await waitForCheckpoint(application, "waiting-for-isolated-settings", () => {
      const settings = observeSettings(environment);
      const observation = observeDatabase(environment, snapshot);
      return settings.directPrimary
        && countSessions(observation, (session) => session.provider_id === "local-openai-compatible" && session.status === "completed") >= 1;
    }, deadlineAt);
    await waitForCheckpoint(application, "enabling-larm-canary", () => observeSettings(environment).larmPrimary, deadlineAt);
    await stopApplication(application, knownDatabaseIdentifiersOrEmpty(environment, snapshot));
    application = undefined;

    application = await startApplication(environment, true);
    const larmId = observeSettings(environment).larmId;
    await waitForCheckpoint(application, "waiting-for-ui-workload", () => {
      const observation = observeDatabase(environment, snapshot);
      return countSessions(observation, (session) => session.provider_id === larmId && session.status === "completed") >= 1
        && countSessions(observation, (session) => session.provider_id === larmId && session.status === "cancelled") >= 1;
    }, deadlineAt);
    await waitForCheckpoint(application, "checkpoint-timeout-fixture-ready", () => {
      const observation = observeDatabase(environment, snapshot);
      return countSessions(observation, (session) => session.provider_id === larmId && session.failure_kind === "timeout") >= 1
        && countSessions(observation, (session) => session.provider_id === "local-openai-compatible" && session.status === "completed") >= 2;
    }, deadlineAt);
    await waitForCheckpoint(application, "checkpoint-tunnel-interruption-ready", () => {
      const observation = observeDatabase(environment, snapshot);
      return countSessions(observation, (session) => session.provider_id === larmId && session.failure_kind === "partial-output") >= 1
        && countSessions(observation, (session) => session.provider_id === larmId && session.status === "completed") >= 2;
    }, deadlineAt);
    await waitForCheckpoint(application, "checkpoint-larm-restart-ready", () => {
      const observation = observeDatabase(environment, snapshot);
      return countSessions(observation, (session) => session.provider_id === larmId && session.status === "completed") >= 3;
    }, deadlineAt);
    await waitForCheckpoint(application, "checkpoint-capacity-fixture-ready", () => {
      const observation = observeDatabase(environment, snapshot);
      return countSessions(observation, (session) => session.provider_id === larmId && session.failure_kind === "capacity") >= 1
        && countSessions(observation, (session) => session.provider_id === "local-openai-compatible" && session.status === "completed") >= 3;
    }, deadlineAt);

    const rust = await runRustLiveSuite("functional", environment, identity, deadlineAt);
    const rustFailure = reportFailure(rust);
    if (rustFailure !== undefined) throw rustFailure;
    await waitForCheckpoint(application, "waiting-for-ui-workload", () => {
      const settings = observeSettings(environment);
      const observation = observeDatabase(environment, snapshot);
      return settings.directPrimary
        && countSessions(observation, (session) => session.provider_id === "local-openai-compatible" && session.status === "completed") >= 6;
    }, deadlineAt);
    await waitForCheckpoint(application, "enabling-larm-canary", () => observeSettings(environment).larmPrimary, deadlineAt);
    await stopApplication(application, knownDatabaseIdentifiersOrEmpty(environment, snapshot));
    application = undefined;

    fixedProgress("checkpoint-saaa-restart");
    application = await startApplication(environment, false);
    await waitForCheckpoint(application, "waiting-for-ui-workload", () => {
      const observation = observeDatabase(environment, snapshot);
      return countSessions(observation, (session) => session.provider_id === "local-openai-compatible" && session.status === "completed") >= 9;
    }, deadlineAt);
    await stopApplication(application, knownDatabaseIdentifiersOrEmpty(environment, snapshot));
    application = undefined;

    const observation = observeDatabase(environment, snapshot);
    validateFunctionalObservation(observation, larmId);
    const local = emptyReport({
      saaaCommit: identity.saaaCommit,
      artifactSha256: identity.artifactSha256,
      manifestSha256: environment.manifestSha256,
      larmContractCommit: environment.deployedCommit,
      deploymentRevision: environment.deploymentRevision,
    }, "functional");
    Object.assign(local.scenarioCounts, {
      normalTurns: 3,
      cancellations: 1,
      requestTimeouts: 1,
      partialInterruptions: 1,
      larmRestarts: 1,
      saaaRestarts: 1,
      capacityRejections: 1,
      rollbackPreflightTurns: 1,
      settingsRollbackTurns: 3,
      killSwitchRollbackTurns: 3,
    });
    Object.assign(local.resultCounts, {
      completed: 12,
      cancelled: 1,
      expectedFailures: 3,
      explicitProviderFallbacks: 2,
    });
    return { local, rust };
  } catch (error) {
    primaryError = error;
    throw error;
  } finally {
    if (application !== undefined) {
      try {
        await stopApplication(application, knownDatabaseIdentifiersOrEmpty(environment, snapshot));
      } catch (cleanupError) {
        if (
          primaryError === undefined
          || (cleanupError instanceof RunnerError
            && cleanupError.errorCode === "redaction-failed"
            && (!(primaryError instanceof RunnerError) || primaryError.errorCode !== "redaction-failed"))
        ) {
          throw cleanupError;
        }
      }
    }
  }
}

async function readStreamBytes(stream: ReadableStream<Uint8Array>, limit: number): Promise<Buffer> {
  const reader = stream.getReader();
  const chunks: Buffer[] = [];
  let size = 0;
  try {
    while (true) {
      const next = await reader.read();
      if (next.done) break;
      size += next.value.byteLength;
      if (size > limit) throw new RunnerError(2, "rss-growth", "failed");
      chunks.push(Buffer.from(next.value));
    }
  } finally {
    reader.releaseLock();
  }
  return Buffer.concat(chunks);
}

async function sampleRssKiB(pid: number): Promise<number> {
  const child = Bun.spawn(["/bin/ps", "-o", "rss=", "-p", String(pid)], {
    cwd: ROOT,
    env: {},
    stdout: "pipe",
    stderr: "pipe",
  });
  const stdout = readStreamBytes(child.stdout, 64);
  const stderr = readStreamBytes(child.stderr, 64);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      void terminateOwnedChild(child).finally(() => reject(new RunnerError(2, "sampling-gap", "failed")));
    }, 2_000);
  });
  try {
    let exitCode: number;
    try {
      exitCode = await Promise.race([child.exited, deadline]);
    } catch (error) {
      await Promise.allSettled([stdout, stderr]);
      throw error;
    }
    const [output, error] = await Promise.all([stdout, stderr]);
    const value = output.toString("ascii").trim();
    if (exitCode !== 0 || error.length !== 0 || !/^\d{1,16}$/.test(value)) {
      throw new RunnerError(2, "rss-growth", "failed");
    }
    const rssKiB = Number(value);
    if (!Number.isSafeInteger(rssKiB) || rssKiB < 0 || rssKiB > 1_073_741_824) {
      throw new RunnerError(2, "rss-growth", "failed");
    }
    return rssKiB;
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

function median(values: number[]): number {
  if (values.length === 0) throw new RunnerError(2, "sampling-gap", "failed");
  const ordered = [...values].sort((left, right) => left - right);
  const middle = Math.floor(ordered.length / 2);
  return ordered.length % 2 === 0 ? (ordered[middle - 1]! + ordered[middle]!) / 2 : ordered[middle]!;
}

export function validateSoakObservation(observation: DatabaseObservation, larmId: string): { completed: number; cancelled: number } {
  const completed = observation.runs.filter((run) => run.status === "completed").length;
  const cancelled = observation.runs.filter((run) => run.status === "cancelled").length;
  const sessions = observation.sessions.filter((session) => session.provider_id === larmId && session.provider_kind === "larm");
  const allocationIds = sessions.flatMap((session) => session.allocation_id === null ? [] : [session.allocation_id]);
  const runIds = new Set(observation.runs.map((run) => run.id));
  const sessionRunIds = sessions.flatMap((session) => session.runtime_run_id === null ? [] : [session.runtime_run_id]);
  if (
    completed + cancelled !== observation.runs.length
    || sessions.length !== observation.runs.length
    || observation.runs.some((run) => run.provider_id !== larmId)
    || sessionRunIds.length !== sessions.length
    || new Set(sessionRunIds).size !== sessionRunIds.length
    || sessionRunIds.some((runId) => !runIds.has(runId))
    || sessions.some((session) => {
      const run = observation.runs.find((candidate) => candidate.id === session.runtime_run_id);
      return run === undefined || run.status !== session.status;
    })
    || sessions.some((session) => !["completed", "cancelled"].includes(session.status))
    || sessions.some((session) => session.fallback_used !== 0
      || session.output_started !== 1
      || (session.status === "completed" ? session.failure_kind !== null : session.failure_kind !== "cancelled"))
    || sessions.some((session) => session.selected_runtime_id === null
      || runtimeCategory(session.selected_runtime_id) !== "resident-default")
    || sessions.some((session) => session.route_id !== "llm-default" || session.selection_reason !== "primary")
    || sessions.some((session) => session.allocation_id === null || !["released", "deferred-to-ttl"].includes(session.release_status))
    || new Set(allocationIds).size !== allocationIds.length
  ) {
    throw new RunnerError(2, "runtime-policy-violation", "failed");
  }
  return { completed, cancelled };
}

async function observeSoak(
  mode: "soak-30m" | "soak-2h",
  environment: ValidatedEnvironment,
  identity: { artifactSha256: string; saaaCommit: string },
  deadlineAt: number,
): Promise<{ local: ReadinessReport; rust: ReadinessReport }> {
  const durationMs = mode === "soak-30m" ? 30 * 60_000 : 2 * 60 * 60_000;
  const minimumNormal = mode === "soak-30m" ? 20 : 60;
  const minimumCancelled = mode === "soak-30m" ? 5 : 10;
  const snapshot = withCanaryDatabase(environment.dataDirectory, databaseSnapshot);
  const settings = observeSettings(environment);
  if (!settings.larmPrimary) throw new RunnerError(3, "gate-missing", "blocked");
  const larmId = settings.larmId;
  let application: OwnedApplication | undefined = await startApplication(environment, true);
  const abort = new AbortController();
  const rustPromise = runRustLiveSuite(mode, environment, identity, deadlineAt, abort.signal);
  let rustFailure: unknown;
  void rustPromise.then(
    (report) => {
      rustFailure = reportFailure(report);
    },
    (error: unknown) => {
      rustFailure = error;
    },
  );
  const started = performance.now();
  let nextSampleAt = started;
  let lastRssAt = started;
  let lastDatabaseAt = started;
  let maximumSamplingGapMs = 0;
  let restartCheckpointEmitted = false;
  let localRestartCompleted = false;
  const firstProcessRss: Array<{ elapsedMs: number; rssKiB: number }> = [];
  let primaryError: unknown;
  fixedProgress("waiting-for-ui-workload");
  try {
    while (performance.now() - started < durationMs) {
      if (rustFailure !== undefined) throw rustFailure;
      const now = performance.now();
      if (now < nextSampleAt) await Bun.sleep(nextSampleAt - now);
      const elapsedMs = performance.now() - started;
      if (application.child.exitCode !== null || applicationDetectedForbiddenData(application)) {
        throw new RunnerError(2, applicationDetectedForbiddenData(application) ? "redaction-failed" : "restart-recovery-failed", "failed");
      }
      const rssKiB = await sampleRssKiB(application.child.pid);
      const rssAt = performance.now();
      maximumSamplingGapMs = Math.max(maximumSamplingGapMs, rssAt - lastRssAt);
      lastRssAt = rssAt;
      const observation = observeDatabase(environment, snapshot);
      const databaseAt = performance.now();
      maximumSamplingGapMs = Math.max(maximumSamplingGapMs, databaseAt - lastDatabaseAt);
      lastDatabaseAt = databaseAt;
      if (!localRestartCompleted) firstProcessRss.push({ elapsedMs, rssKiB });
      if (mode === "soak-2h" && !restartCheckpointEmitted && elapsedMs >= 30 * 60_000) {
        if (observation.runs.some((run) => run.status === "running")) {
          nextSampleAt += 5_000;
          continue;
        }
        fixedProgress("checkpoint-larm-restart-ready");
        restartCheckpointEmitted = true;
      }
      if (mode === "soak-2h" && !localRestartCompleted && elapsedMs >= 70 * 60_000) {
        if (observation.runs.some((run) => run.status === "running")) {
          nextSampleAt += 5_000;
          continue;
        }
        fixedProgress("checkpoint-saaa-restart");
        await stopApplication(application, knownObservationIdentifiers(observation));
        application = undefined;
        application = await startApplication(environment, true);
        assertLarmPrimarySettings(environment, larmId);
        localRestartCompleted = true;
        const restartedAt = performance.now();
        maximumSamplingGapMs = Math.max(
          maximumSamplingGapMs,
          restartedAt - lastRssAt,
          restartedAt - lastDatabaseAt,
        );
        lastRssAt = restartedAt;
        lastDatabaseAt = restartedAt;
      }
      nextSampleAt += 5_000;
      if (nextSampleAt < performance.now() - 15_000) throw new RunnerError(2, "sampling-gap", "failed");
    }
    if (mode === "soak-2h" && (!restartCheckpointEmitted || !localRestartCompleted)) {
      throw new RunnerError(2, "restart-recovery-failed", "failed");
    }
    let observation = observeDatabase(environment, snapshot);
    assertLarmPrimarySettings(environment, larmId);
    if (observation.runs.some((run) => run.status === "running")) {
      throw new RunnerError(2, "database-schema-invalid", "failed");
    }
    await stopApplication(application, knownObservationIdentifiers(observation));
    application = undefined;
    observation = observeDatabase(environment, snapshot);
    if (observation.runs.some((run) => run.status === "running")) throw new RunnerError(2, "database-schema-invalid", "failed");
    const workload = validateSoakObservation(observation, larmId);
    if (workload.completed < minimumNormal || workload.cancelled < minimumCancelled) {
      throw new RunnerError(2, "report-schema-invalid", "failed");
    }
    const memoryStart = 10 * 60_000;
    const memoryEnd = mode === "soak-30m" ? 30 * 60_000 : 70 * 60_000;
    const memory = firstProcessRss.filter((sample) => sample.elapsedMs >= memoryStart && sample.elapsedMs <= memoryEnd);
    if (memory.length === 0) throw new RunnerError(2, "sampling-gap", "failed");
    const rssRangeMiB = Math.ceil((Math.max(...memory.map((sample) => sample.rssKiB)) - Math.min(...memory.map((sample) => sample.rssKiB))) / 1_024);
    let previousMedianMiB = 0;
    let lastMedianMiB = 0;
    if (mode === "soak-2h") {
      previousMedianMiB = Math.ceil(median(firstProcessRss.filter((sample) => sample.elapsedMs >= 10 * 60_000 && sample.elapsedMs < 40 * 60_000).map((sample) => sample.rssKiB)) / 1_024);
      lastMedianMiB = Math.ceil(median(firstProcessRss.filter((sample) => sample.elapsedMs >= 40 * 60_000 && sample.elapsedMs <= 70 * 60_000).map((sample) => sample.rssKiB)) / 1_024);
    }
    const local = emptyReport({
      saaaCommit: identity.saaaCommit,
      artifactSha256: identity.artifactSha256,
      manifestSha256: environment.manifestSha256,
      larmContractCommit: environment.deployedCommit,
      deploymentRevision: environment.deploymentRevision,
    }, mode);
    local.scenarioCounts.normalTurns = workload.completed;
    local.scenarioCounts.cancellations = workload.cancelled;
    local.scenarioCounts.larmRestarts = mode === "soak-2h" ? 1 : 0;
    local.scenarioCounts.saaaRestarts = mode === "soak-2h" ? 1 : 0;
    local.resultCounts.completed = workload.completed;
    local.resultCounts.cancelled = workload.cancelled;
    local.timingSummary.elapsedMs = Math.min(10_800_000, Math.ceil(performance.now() - started));
    local.timingSummary.sampleIntervalSeconds = 5;
    local.timingSummary.rssMaxSamplingGapSeconds = Math.ceil(maximumSamplingGapMs / 1_000);
    local.resourceSummary.rssRangeMiB = rssRangeMiB;
    local.resourceSummary.rssPrevious30mMedianMiB = previousMedianMiB;
    local.resourceSummary.rssLast30mMedianMiB = lastMedianMiB;
    const rust = await rustPromise;
    const completedRustFailure = reportFailure(rust);
    if (completedRustFailure !== undefined) throw completedRustFailure;
    return { local, rust };
  } catch (error) {
    primaryError = error;
    abort.abort();
    await Promise.allSettled([rustPromise]);
    throw error;
  } finally {
    if (application !== undefined) {
      try {
        await stopApplication(application, knownDatabaseIdentifiersOrEmpty(environment, snapshot));
      } catch (cleanupError) {
        if (
          primaryError === undefined
          || (cleanupError instanceof RunnerError
            && cleanupError.errorCode === "redaction-failed"
            && (!(primaryError instanceof RunnerError) || primaryError.errorCode !== "redaction-failed"))
        ) {
          throw cleanupError;
        }
      }
    }
  }
}

function encodeU32(value: number): Buffer {
  const buffer = Buffer.alloc(4);
  buffer.writeUInt32BE(value);
  return buffer;
}

function encodeU64(value: number): Buffer {
  if (!Number.isSafeInteger(value) || value < 0) throw new RunnerError(2, "artifact-mismatch", "failed");
  const buffer = Buffer.alloc(8);
  buffer.writeBigUInt64BE(BigInt(value));
  return buffer;
}

interface BundleFile {
  filename: string;
  relativePath: Buffer;
  size: number;
  identity: StableFileIdentity;
}

function collectBundleFiles(directory: string, prefix = ""): BundleFile[] {
  assertNoSymlinkComponents(directory);
  const directoryInfo = lstatSync(directory);
  if (!directoryInfo.isDirectory() || directoryInfo.isSymbolicLink()) {
    throw new RunnerError(2, "artifact-mismatch", "failed");
  }
  const result: BundleFile[] = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const filename = join(directory, entry.name);
    const relativeName = prefix === "" ? entry.name : `${prefix}/${entry.name}`;
    assertNoSymlinkComponents(filename);
    const info = lstatSync(filename);
    if (info.isSymbolicLink()) throw new RunnerError(2, "artifact-mismatch", "failed");
    if (info.isDirectory()) {
      result.push(...collectBundleFiles(filename, relativeName));
    } else if (info.isFile() && info.nlink === 1 && Number.isSafeInteger(info.size)) {
      const relativePath = Buffer.from(relativeName, "utf8");
      if (relativePath.toString("utf8") !== relativeName || relativePath.length > 0xffff_ffff) {
        throw new RunnerError(2, "artifact-mismatch", "failed");
      }
      result.push({ filename, relativePath, size: info.size, identity: stableFileIdentity(info) });
    } else {
      throw new RunnerError(2, "artifact-mismatch", "failed");
    }
  }
  return result;
}

function hashBundleFile(file: BundleFile): Buffer {
  let descriptor: number | undefined;
  try {
    assertNoSymlinkComponents(file.filename);
    descriptor = openSync(file.filename, fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW);
    const beforeInfo = fstatSync(descriptor);
    const before = stableFileIdentity(beforeInfo);
    if (!beforeInfo.isFile() || beforeInfo.isSymbolicLink() || before.nlink !== 1 || !sameFileIdentity(before, file.identity)) {
      throw new RunnerError(2, "artifact-mismatch", "failed");
    }
    const hash = createHash("sha256");
    const buffer = Buffer.allocUnsafe(64 * 1_024);
    let total = 0;
    while (true) {
      const count = readSync(descriptor, buffer, 0, buffer.length, null);
      if (count === 0) break;
      total += count;
      if (total > file.size) throw new RunnerError(2, "artifact-mismatch", "failed");
      hash.update(buffer.subarray(0, count));
    }
    const after = stableFileIdentity(fstatSync(descriptor));
    if (total !== file.size || !sameFileIdentity(before, after)) {
      throw new RunnerError(2, "artifact-mismatch", "failed");
    }
    return hash.digest();
  } catch {
    throw new RunnerError(2, "artifact-mismatch", "failed");
  } finally {
    if (descriptor !== undefined) closeSync(descriptor);
  }
}

function sameBundleInventory(left: BundleFile[], right: BundleFile[]): boolean {
  return left.length === right.length && left.every((file, index) => {
    const other = right[index];
    return other !== undefined
      && file.relativePath.equals(other.relativePath)
      && sameFileIdentity(file.identity, other.identity);
  });
}

export async function canonicalBundleDigest(bundle = RELEASE_BUNDLE): Promise<string> {
  try {
    assertNoSymlinkComponents(bundle);
    const rootInfo = lstatSync(bundle);
    if (!rootInfo.isDirectory() || rootInfo.isSymbolicLink()) throw new RunnerError(2, "artifact-mismatch", "failed");
    const executable = bundle === RELEASE_BUNDLE ? RELEASE_EXECUTABLE : join(bundle, "Contents/MacOS/saaa");
    const executableInfo = lstatSync(executable);
    if (!executableInfo.isFile() || executableInfo.isSymbolicLink() || executableInfo.nlink !== 1 || (executableInfo.mode & 0o111) === 0) {
      throw new RunnerError(2, "artifact-mismatch", "failed");
    }
    const files = collectBundleFiles(bundle).sort((left, right) => Buffer.compare(left.relativePath, right.relativePath));
    if (files.length === 0) throw new RunnerError(2, "artifact-mismatch", "failed");
    const outer = createHash("sha256").update(Buffer.from("SAAA-BUNDLE-V1\0", "ascii"));
    for (const file of files) {
      outer.update(encodeU32(file.relativePath.length));
      outer.update(file.relativePath);
      outer.update(encodeU64(file.size));
      outer.update(hashBundleFile(file));
    }
    const finalFiles = collectBundleFiles(bundle).sort((left, right) => Buffer.compare(left.relativePath, right.relativePath));
    if (!sameBundleInventory(files, finalFiles)) throw new RunnerError(2, "artifact-mismatch", "failed");
    return outer.digest("hex");
  } catch {
    throw new RunnerError(2, "artifact-mismatch", "failed");
  }
}

export function atomicWriteReport(filename: string, reportInput: ReadinessReport): void {
  const report = validateReport(reportInput);
  const bytes = Buffer.from(`${JSON.stringify(report)}\n`);
  if (bytes.length > MAX_REPORT_BYTES || existsSync(filename)) throw new RunnerError(2, "report-schema-invalid", "failed");
  assertNoSymlinkComponents(dirname(filename));
  const temporary = join(dirname(filename), `.${basename(filename)}.${randomBytes(16).toString("hex")}.tmp`);
  try {
    const descriptor = openSync(temporary, fsConstants.O_WRONLY | fsConstants.O_CREAT | fsConstants.O_EXCL | fsConstants.O_NOFOLLOW, 0o600);
    try {
      writeFileSync(descriptor, bytes);
      fsyncSync(descriptor);
    } finally {
      closeSync(descriptor);
    }
    chmodSync(temporary, 0o600);
    // A same-directory hard link publishes the complete inode atomically and fails if target exists.
    // It provides the no-overwrite guarantee that portable rename(2) lacks.
    linkSync(temporary, filename);
  } catch {
    throw new RunnerError(2, "report-schema-invalid", "failed");
  } finally {
    if (existsSync(temporary)) unlinkSync(temporary);
  }
}

function readFinalReportWithBytes(filename: string): { report: ReadinessReport; bytes: Buffer } {
  const failure = () => new RunnerError(2, "report-schema-invalid", "failed");
  const { bytes } = readBoundedRegularFile(filename, MAX_REPORT_BYTES, failure, 0o600);
  return { report: validateReport(parseJsonBytes(bytes, failure)), bytes };
}

function readFinalReport(filename: string): ReadinessReport {
  return readFinalReportWithBytes(filename).report;
}

function resultStrength(result: Result): number {
  return { passed: 0, blocked: 1, failed: 2 }[result];
}

export function aggregateReports(inputs: ReadinessReport[]): ReadinessReport {
  if (inputs.length !== 4) throw new RunnerError(2, "report-schema-invalid", "failed");
  const reports = inputs.map(validateReport);
  const modes: LiveMode[] = ["preflight", "functional", "soak-30m", "soak-2h"];
  if (modes.some((mode, index) => reports[index]!.mode !== mode)) throw new RunnerError(2, "report-schema-invalid", "failed");
  const first = reports[0]!;
  for (const report of reports.slice(1)) {
    for (const key of ["saaaCommit", "saaaArtifactSha256", "canaryManifestSha256", "larmContractCommit", "deploymentRevision"] as const) {
      if (report[key] !== first[key]) throw new RunnerError(2, "report-schema-invalid", "failed");
    }
  }
  const allocationBaselineValid = reports.every((report) =>
    report.resourceSummary.baselineActiveAllocations === first.resourceSummary.baselineActiveAllocations
    && report.resourceSummary.finalActiveAllocations === first.resourceSummary.baselineActiveAllocations
  );
  const sum = <T extends readonly string[]>(keys: T, getter: (report: ReadinessReport) => Record<T[number], number>) => Object.fromEntries(keys.map((key) => {
    const value = reports.reduce((total, report) => total + getter(report)[key], 0);
    if (!Number.isSafeInteger(value) || value > 10_000) throw new RunnerError(2, "report-schema-invalid", "failed");
    return [key, value];
  })) as Record<T[number], number>;
  const sourceResult = reports.reduce((strongest, report) => resultStrength(report.result) > resultStrength(strongest) ? report.result : strongest, "passed" as Result);
  const result: Result = allocationBaselineValid ? sourceResult : "failed";
  const aggregate: ReadinessReport = {
    ...first,
    startedAt: reports.reduce((earliest, report) => utcTimestampSortKey(report.startedAt) < utcTimestampSortKey(earliest) ? report.startedAt : earliest, first.startedAt),
    finishedAt: reports.reduce((latest, report) => utcTimestampSortKey(report.finishedAt) > utcTimestampSortKey(latest) ? report.finishedAt : latest, first.finishedAt),
    mode: "aggregate",
    scenarioCounts: sum(SCENARIO_KEYS, (report) => report.scenarioCounts),
    resultCounts: sum(RESULT_KEYS, (report) => report.resultCounts),
    timingSummary: Object.fromEntries(TIMING_KEYS.map((key) => [key, Math.max(...reports.map((report) => report.timingSummary[key]))])) as ReadinessReport["timingSummary"],
    resourceSummary: {
      baselineActiveAllocations: first.resourceSummary.baselineActiveAllocations,
      maxActiveAllocations: Math.max(...reports.map((report) => report.resourceSummary.maxActiveAllocations)),
      finalActiveAllocations: reports[3]!.resourceSummary.finalActiveAllocations,
      rssRangeMiB: reports[3]!.resourceSummary.rssRangeMiB,
      rssPrevious30mMedianMiB: reports[3]!.resourceSummary.rssPrevious30mMedianMiB,
      rssLast30mMedianMiB: reports[3]!.resourceSummary.rssLast30mMedianMiB,
    },
    leaseSummary: reports[1]!.leaseSummary,
    failureCodes: [...new Set([
      ...reports.flatMap((report) => report.failureCodes),
      ...(allocationBaselineValid ? [] : ["allocation-leak" as const]),
    ])],
    redactionCheck: reports.some((report) => report.redactionCheck === "failed") ? "failed" : "passed",
    result,
  };
  return validateReport(aggregate);
}

function assertPredecessor(reportDirectory: string, mode: LiveMode, identity: Omit<ValidatedEnvironment, "token" | "rollbackCredential" | "manifest" | "manifestFile" | "baseUrl" | "dataDirectory" | "reportDirectory"> & { artifactSha256: string; saaaCommit: string }): ReadinessReport | undefined {
  const predecessors: LiveMode[] = mode === "preflight"
    ? []
    : mode === "functional"
      ? ["preflight"]
      : mode === "soak-30m"
        ? ["preflight", "functional"]
        : ["preflight", "functional", "soak-30m"];
  if (predecessors.length === 0) return undefined;
  const reports = predecessors.map((expectedMode) => ({
    expectedMode,
    report: evaluateReport(readFinalReport(join(reportDirectory, REPORT_FILENAMES[expectedMode]))),
  }));
  const baseline = reports[0]!.report.resourceSummary.baselineActiveAllocations;
  for (const { expectedMode, report } of reports) {
    if (
      report.mode !== expectedMode
      || report.result !== "passed"
      || report.saaaCommit !== identity.saaaCommit
      || report.saaaArtifactSha256 !== identity.artifactSha256
      || report.canaryManifestSha256 !== identity.manifestSha256
      || report.larmContractCommit !== identity.deployedCommit
      || report.deploymentRevision !== identity.deploymentRevision
      || report.resourceSummary.baselineActiveAllocations !== baseline
      || report.resourceSummary.finalActiveAllocations !== baseline
    ) {
      throw new RunnerError(3, "gate-missing", "blocked");
    }
  }
  return reports.at(-1)!.report;
}

function fixedProgress(code: string): void {
  process.stderr.write(`${code}\n`);
}

function testNameForMode(mode: LiveMode): string {
  return {
    preflight: "providers::larm::live_canary::live_preflight",
    functional: "providers::larm::live_canary::live_functional",
    "soak-30m": "providers::larm::live_canary::observe_soak_30m",
    "soak-2h": "providers::larm::live_canary::observe_soak_2h",
  }[mode];
}

async function runRustLiveSuite(mode: LiveMode, environment: ValidatedEnvironment, identity: { artifactSha256: string; saaaCommit: string }, deadlineAt: number, signal?: AbortSignal): Promise<ReadinessReport> {
  const resultFilename = join(environment.reportDirectory, RUST_FRAGMENT_FILENAMES[mode]);
  if (existsSync(resultFilename)) throw new RunnerError(3, "environment-invalid", "blocked");
  const scanner = new ForbiddenDataScanner([
    environment.token,
    environment.rollbackCredential,
    environment.baseUrl,
    environment.manifest.rollbackProvider.endpoint,
    "Reply with exactly: CANARY_OK",
    "List the numbers 1 through 5.",
    "Write one short greeting in Japanese.",
    "Write ten numbered words, one at a time.",
    "Reply with exactly: READY",
  ]);
  const modeDeadline = mode === "preflight" ? 30 * 60_000 : mode === "functional" ? 60 * 60_000 : mode === "soak-30m" ? 45 * 60_000 : 150 * 60_000;
  const child = await runBoundedChild({
    command: ["cargo", "test", "--manifest-path", "src-tauri/Cargo.toml", testNameForMode(mode), "--", "--ignored", "--exact", "--test-threads=1"],
    environment: rustChildEnvironment(process.env, {
      resultFile: resultFilename,
      artifactSha256: identity.artifactSha256,
      manifestSha256: environment.manifestSha256,
      metricsScope: environment.manifest.metricsScope,
      saaaCommit: identity.saaaCommit,
    }),
    limit: MAX_CHILD_BYTES,
    deadlineMs: Math.max(1, Math.min(modeDeadline, deadlineAt - performance.now())),
    scanner,
    ...(signal === undefined ? {} : { signal }),
  });
  if (child.redactionFailed) throw new RunnerError(2, "redaction-failed", "failed");
  if (child.exitCode !== 0 || !existsSync(resultFilename)) throw new RunnerError(2, "internal", "failed");
  return readFinalReport(resultFilename);
}

async function releaseArtifact(environment: ValidatedEnvironment, command: CliArguments["command"], deadlineAt: number): Promise<{ artifactSha256: string; saaaCommit: string }> {
  const saaaCommit = assertCleanRepository();
  if (command === "preflight") {
    fixedProgress("building-release-artifact");
    const scanner = new ForbiddenDataScanner([environment.token, environment.rollbackCredential, environment.baseUrl, environment.manifest.rollbackProvider.endpoint]);
    const build = await runBoundedChild({
      command: ["bunx", "tauri", "build", "--bundles", "app"],
      environment: buildEnvironment(process.env),
      limit: MAX_BUILD_BYTES,
      deadlineMs: Math.max(1, Math.min(25 * 60_000, deadlineAt - performance.now())),
      scanner,
    });
    if (build.redactionFailed) throw new RunnerError(2, "redaction-failed", "failed");
    if (build.exitCode !== 0) throw new RunnerError(2, "artifact-mismatch", "failed");
    if (assertCleanRepository() !== saaaCommit) throw new RunnerError(2, "artifact-mismatch", "failed");
  }
  const artifactSha256 = await canonicalBundleDigest();
  return { artifactSha256, saaaCommit };
}

function removeRustFragment(filename: string): void {
  try {
    const info = lstatSync(filename);
    if (!info.isFile() || info.isSymbolicLink() || info.nlink !== 1 || modeBits(info.mode) !== 0o600) {
      throw new RunnerError(2, "report-schema-invalid", "failed");
    }
    assertCurrentOwner(filename);
    unlinkSync(filename);
  } catch (error) {
    if (error instanceof RunnerError) throw error;
    throw new RunnerError(2, "report-schema-invalid", "failed");
  }
}

function failureCode(error: RunnerError): ReadinessReport["failureCodes"][number] {
  return FAILURE_CODES.includes(error.errorCode as ReadinessReport["failureCodes"][number])
    ? error.errorCode as ReadinessReport["failureCodes"][number]
    : "internal";
}

async function executeLive(arguments_: CliArguments): Promise<{ mode: LiveMode; result: Result }> {
  const mode: LiveMode = arguments_.command === "preflight"
    ? "preflight"
    : arguments_.command === "canary"
      ? "functional"
      : arguments_.duration === "30m" ? "soak-30m" : "soak-2h";
  const totalDeadline = mode === "preflight" ? 30 * 60_000 : mode === "functional" ? 60 * 60_000 : mode === "soak-30m" ? 45 * 60_000 : 150 * 60_000;
  const commandStarted = performance.now();
  const commandStartedAt = new Date().toISOString();
  const deadlineAt = commandStarted + totalDeadline;
  const reportDirectory = validateReportDirectory(arguments_.reportDirectory, arguments_.command, arguments_.duration);
  const environment = validateLiveEnvironment(
    reportDirectory,
    mode === "preflight" ? "preflight" : mode === "functional" ? "functional" : "later",
  );
  const artifact = await releaseArtifact(environment, arguments_.command, deadlineAt);
  const identity = {
    saaaCommit: artifact.saaaCommit,
    artifactSha256: artifact.artifactSha256,
    manifestSha256: environment.manifestSha256,
    larmContractCommit: environment.deployedCommit,
    deploymentRevision: environment.deploymentRevision,
  };
  const finalFilename = join(reportDirectory, REPORT_FILENAMES[mode]);
  const fragmentFilename = join(reportDirectory, RUST_FRAGMENT_FILENAMES[mode]);
  try {
    assertPredecessor(reportDirectory, mode, { ...environment, ...artifact });
    let rust: ReadinessReport;
    let local: ReadinessReport;
    if (mode === "preflight") {
      rust = await runRustLiveSuite(mode, environment, artifact, deadlineAt);
      local = emptyReport(identity, mode);
    } else if (mode === "functional") {
      ({ rust, local } = await observeFunctional(environment, artifact, deadlineAt));
    } else {
      ({ rust, local } = await observeSoak(mode, environment, artifact, deadlineAt));
    }
    local.startedAt = commandStartedAt;
    local.finishedAt = new Date().toISOString();
    local.timingSummary.elapsedMs = Math.min(
      10_800_000,
      Math.max(local.timingSummary.elapsedMs, Math.ceil(performance.now() - commandStarted)),
    );
    const merged = mergeReports(rust, local);
    fixedProgress("finalizing-report");
    removeRustFragment(fragmentFilename);
    atomicWriteReport(finalFilename, merged);
    return { mode, result: merged.result };
  } catch (error) {
    const known = error instanceof RunnerError ? error : new RunnerError(70, "internal", "failed");
    if (!existsSync(finalFilename)) {
      const failure = emptyReport(identity, mode, known.result, [failureCode(known)]);
      failure.startedAt = commandStartedAt;
      failure.finishedAt = new Date().toISOString();
      failure.timingSummary.elapsedMs = Math.min(10_800_000, Math.ceil(performance.now() - commandStarted));
      atomicWriteReport(finalFilename, failure);
    }
    if (existsSync(fragmentFilename)) {
      try {
        removeRustFragment(fragmentFilename);
      } catch {
        // Preserve an untrusted or malformed fragment for operator inspection; never unlink it blindly.
      }
    }
    throw known;
  }
}

async function executeReport(arguments_: CliArguments): Promise<{ mode: "aggregate"; result: Result }> {
  const reportDirectory = validateReportDirectory(arguments_.reportDirectory, "report");
  const filenames = (["preflight", "functional", "soak-30m", "soak-2h"] as const).map((mode) => join(reportDirectory, REPORT_FILENAMES[mode]));
  const inputs = filenames.map(readFinalReportWithBytes);
  const inputHashes = inputs.map(({ bytes }) => createHash("sha256").update(bytes).digest("hex"));
  if (new Set(inputHashes).size !== inputHashes.length) throw new RunnerError(2, "report-schema-invalid", "failed");
  const reports = inputs.map(({ report }) => evaluateReport(report));
  const aggregate = aggregateReports(reports);
  const scanner = new ForbiddenDataScanner([
    process.env.LARM_API_TOKEN,
    process.env.SAAA_LARM_CANARY_BASE_URL,
    process.env.SAAA_PROVIDER_LOCAL_OPENAI_COMPATIBLE_API_KEY,
  ]);
  scanner.scan(Buffer.from(JSON.stringify(aggregate)));
  if (scanner.detected) throw new RunnerError(2, "redaction-failed", "failed");
  atomicWriteReport(join(reportDirectory, REPORT_FILENAMES.aggregate), aggregate);
  return { mode: "aggregate", result: aggregate.result };
}

export async function run(arguments_: CliArguments): Promise<{ mode: ReportMode; result: Result }> {
  if (arguments_.command === "report") return executeReport(arguments_);
  return executeLive(arguments_);
}

async function main(): Promise<void> {
  let mode: ReportMode = "preflight";
  try {
    const arguments_ = parseCliArguments(process.argv.slice(2));
    mode = arguments_.command === "report" ? "aggregate" : arguments_.command === "canary" ? "functional" : arguments_.command === "soak" ? (arguments_.duration === "30m" ? "soak-30m" : "soak-2h") : "preflight";
    const outcome = await run(arguments_);
    process.stdout.write(`${JSON.stringify({ format: REPORT_FORMAT, mode: outcome.mode, result: outcome.result })}\n`);
    process.exitCode = outcome.result === "passed" ? 0 : outcome.result === "failed" ? 2 : 3;
  } catch (error) {
    const known = error instanceof RunnerError ? error : new RunnerError(70, "internal", "failed");
    process.stderr.write(`${known.errorCode}\n`);
    process.stdout.write(`${JSON.stringify({ format: REPORT_FORMAT, mode, result: known.result })}\n`);
    process.exitCode = known.exitCode;
  }
}

if (import.meta.main) await main();
