import { writeFileSync } from "node:fs";
import { resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import type { InputFingerprint } from "./verify-fingerprint";
import type { WatchDetail } from "./verify-input-watch";
import type { VerificationContext } from "./verify-run";

const ROOT = fileURLToPath(new URL("..", import.meta.url));

export type VerificationRecord = {
  name: string;
  command: string[];
  status: "passed" | "failed";
  durationMs: number;
  lockWaitMs: number;
};

export type AttemptLog = {
  attempt: number;
  durationMs: number;
  records: VerificationRecord[];
  mode: string;
  fallback: string | null;
  executedSteps: string[];
  watcher: WatchDetail | null;
};

export function snapshotAttempt(
  attempt: number,
  durationMs: number,
  records: VerificationRecord[],
  context: VerificationContext,
  watcher: WatchDetail | null,
): AttemptLog {
  return {
    attempt,
    durationMs,
    records: records.map((record) => ({ ...record })),
    mode: context.mode ?? "manual",
    fallback: context.fallback ?? null,
    executedSteps: records.map((record) => record.name),
    watcher,
  };
}

/** Optional machine-readable timings. Success stdout stays a single OK. */
export function writeVerificationReport(
  records: VerificationRecord[],
  status: number,
  durationMs = 0,
  before?: InputFingerprint,
  after?: InputFingerprint,
  attempts = 1,
  context: VerificationContext = {},
  attemptLog: AttemptLog[] = [],
  watch?: WatchDetail,
  cwd = ROOT,
): void {
  const path = process.env.SAAA_VERIFY_REPORT;
  if (!path) return;
  assertReportDestination(path, cwd);
  const measured = Boolean(before?.measured && after?.measured);
  const attemptsWithCurrent = attemptLog.some((entry) => entry.attempt === attempts)
    ? attemptLog
    : [...attemptLog, snapshotAttempt(attempts, durationMs, records, context, watch ?? null)];
  const executedTestCount = records.filter((record) => record.command.includes("test")).length;
  const body = {
    version: 2,
    head: headRevision(cwd),
    status,
    durationMs,
    lockWaitMs: records[0]?.lockWaitMs ?? attemptLog[0]?.records[0]?.lockWaitMs ?? null,
    rss: null,
    requestedMode: context.requestedMode ?? context.mode ?? "manual",
    mode: context.mode ?? "manual",
    level: context.level ?? null,
    fallback: context.fallback ?? null,
    reason: context.reason ?? null,
    candidate: context.candidate ?? [],
    executedSteps: records.map((record) => record.name),
    executedTestCount,
    records,
    attemptLog: attemptsWithCurrent,
    fingerprintMatched: measured && before?.value === after?.value,
    provesInputUnchanged: false,
    fingerprintMeasured: measured,
    attempts,
    watcher: watch?.available ? "attached" : "not attached",
    watcherDetail: watch ?? null,
    unmeasured: watch?.available
      ? ["maxRss", "cpuTime", "compileVersusLink"]
      : ["maxRss", "cpuTime", "compileVersusLink", "watcher"],
  };
  writeFileSync(path, `${JSON.stringify(body)}\n`);
}

function assertReportDestination(path: string, cwd: string): void {
  const resolved = resolve(path);
  const blocked = ["src", "tests", "scripts", "crates", "services", "docs", "src-tauri/src"].map(
    (directory) => resolve(cwd, directory) + sep,
  );
  if (blocked.some((directory) => resolved.startsWith(directory))) {
    throw new Error("verification report must not overwrite source");
  }
}

function headRevision(cwd: string): string | null {
  const result = Bun.spawnSync(["git", "rev-parse", "HEAD"], {
    cwd,
    stdout: "pipe",
    stderr: "pipe",
  });
  if (result.exitCode !== 0) return null;
  const revision = new TextDecoder().decode(result.stdout).trim();
  return revision || null;
}
