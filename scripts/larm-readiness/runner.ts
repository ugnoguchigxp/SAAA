import { createHash } from "node:crypto";
import { existsSync, lstatSync, unlinkSync } from "node:fs";
import { join } from "node:path";
import {
  FAILURE_CODES,
  REPORT_FILENAMES,
  RUST_FRAGMENT_FILENAMES,
  RunnerError,
  emptyReport,
  evaluateReport,
  mergeReports,
  type CliArguments,
  type LiveMode,
  type ReadinessReport,
  type Result,
  type ReportMode,
} from "./schema.ts";
import {
  ForbiddenDataScanner,
  assertCurrentOwner,
  modeBits,
  validateLiveEnvironment,
  validateReportDirectory,
} from "./io.ts";
import { observeFunctional, observeSoak } from "./live.ts";
import {
  aggregateReports,
  assertPredecessor,
  atomicWriteReport,
  fixedProgress,
  readFinalReportWithBytes,
  releaseArtifact,
  runRustLiveSuite,
} from "./bundle.ts";


export function removeRustFragment(filename: string): void {
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

export function failureCode(error: RunnerError): ReadinessReport["failureCodes"][number] {
  return FAILURE_CODES.includes(error.errorCode as ReadinessReport["failureCodes"][number])
    ? error.errorCode as ReadinessReport["failureCodes"][number]
    : "internal";
}

export async function executeLive(arguments_: CliArguments): Promise<{ mode: LiveMode; result: Result }> {
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

export async function executeReport(arguments_: CliArguments): Promise<{ mode: "aggregate"; result: Result }> {
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
