import { createHash, randomBytes } from "node:crypto";
import {
  chmodSync,
  closeSync,
  constants as fsConstants,
  existsSync,
  fstatSync,
  fsyncSync,
  linkSync,
  lstatSync,
  openSync,
  readSync,
  readdirSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, relative, resolve, sep } from "node:path";
import {
  FAILURE_CODES,
  MAX_BUILD_BYTES,
  MAX_CHILD_BYTES,
  MAX_REPORT_BYTES,
  RELEASE_BUNDLE,
  RELEASE_EXECUTABLE,
  REPORT_FILENAMES,
  RESULT_KEYS,
  ROOT,
  RUST_FRAGMENT_FILENAMES,
  RunnerError,
  SCENARIO_KEYS,
  TIMING_KEYS,
  evaluateReport,
  type CliArguments,
  type LiveMode,
  type ReadinessReport,
  type Result,
  utcTimestampSortKey,
  validateReport,
} from "./schema.ts";
import {
  ForbiddenDataScanner,
  assertCleanRepository,
  assertCurrentOwner,
  assertNoSymlinkComponents,
  buildEnvironment,
  modeBits,
  parseJsonBytes,
  readBoundedRegularFile,
  rustChildEnvironment,
  sameFileIdentity,
  stableFileIdentity,
  type StableFileIdentity,
  type ValidatedEnvironment,
} from "./io.ts";
import { runBoundedChild } from "./process.ts";

export function encodeU32(value: number): Buffer {
  const buffer = Buffer.alloc(4);
  buffer.writeUInt32BE(value);
  return buffer;
}

export function encodeU64(value: number): Buffer {
  if (!Number.isSafeInteger(value) || value < 0) throw new RunnerError(2, "artifact-mismatch", "failed");
  const buffer = Buffer.alloc(8);
  buffer.writeBigUInt64BE(BigInt(value));
  return buffer;
}

export interface BundleFile {
  filename: string;
  relativePath: Buffer;
  size: number;
  identity: StableFileIdentity;
}

export function collectBundleFiles(directory: string, prefix = ""): BundleFile[] {
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

export function hashBundleFile(file: BundleFile): Buffer {
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

export function sameBundleInventory(left: BundleFile[], right: BundleFile[]): boolean {
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

export function readFinalReportWithBytes(filename: string): { report: ReadinessReport; bytes: Buffer } {
  const failure = () => new RunnerError(2, "report-schema-invalid", "failed");
  const { bytes } = readBoundedRegularFile(filename, MAX_REPORT_BYTES, failure, 0o600);
  return { report: validateReport(parseJsonBytes(bytes, failure)), bytes };
}

export function readFinalReport(filename: string): ReadinessReport {
  return readFinalReportWithBytes(filename).report;
}

export function resultStrength(result: Result): number {
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

export function assertPredecessor(reportDirectory: string, mode: LiveMode, identity: Omit<ValidatedEnvironment, "token" | "rollbackCredential" | "manifest" | "manifestFile" | "baseUrl" | "dataDirectory" | "reportDirectory"> & { artifactSha256: string; saaaCommit: string }): ReadinessReport | undefined {
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

export function fixedProgress(code: string): void {
  process.stderr.write(`${code}\n`);
}

export function testNameForMode(mode: LiveMode): string {
  return {
    preflight: "providers::larm::live_canary::live_preflight",
    functional: "providers::larm::live_canary::live_functional",
    "soak-30m": "providers::larm::live_canary::observe_soak_30m",
    "soak-2h": "providers::larm::live_canary::observe_soak_2h",
  }[mode];
}

export async function runRustLiveSuite(mode: LiveMode, environment: ValidatedEnvironment, identity: { artifactSha256: string; saaaCommit: string }, deadlineAt: number, signal?: AbortSignal): Promise<ReadinessReport> {
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

export async function releaseArtifact(environment: ValidatedEnvironment, command: CliArguments["command"], deadlineAt: number): Promise<{ artifactSha256: string; saaaCommit: string }> {
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
