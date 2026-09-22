import { createHash } from "node:crypto";
import { existsSync, lstatSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline/promises";
import { z } from "zod";
import {
  type AggregateReport,
  type BuildClass,
  type CaseSpec,
  type Mode,
  type PreflightReport,
  REASON_CODES,
  RESULT_VALUES,
  type ReasonCode,
  type Result,
  RunnerError,
  SCHEMA_VERSION,
  type Suite,
  type SuiteReport,
  aggregateReportSchema,
  caseResultSchema,
  caseSpecs,
  observationSchema,
  preflightReportSchema,
  suiteReportSchema,
} from "./schema.ts";
import {
  DEFAULT_DEVELOPMENT_EXECUTABLE,
  FORBIDDEN_TEXT,
  MAX_REPORT_BYTES,
  ROOT,
  assertCurrentIdentity,
  assertMetric,
  assertVerificationEnvironment,
  commandOutput,
  configuredBundlePath,
  currentIdentity,
  forbiddenDataFindings,
  hashDirectory,
  isWithin,
  isolatedEnvironmentDirectories,
  readJson,
  reportFilename,
  requireDirectory,
  sameIdentity,
  writeJsonExclusive,
} from "./support.ts";

export function runPreflight(reportDirectoryInput: string): PreflightReport {
  const startedAt = new Date().toISOString();
  const reportDirectory = requireDirectory(reportDirectoryInput, true);
  if (isWithin(ROOT, reportDirectory))
    throw new RunnerError(3, "report-directory-inside-repository");
  if (commandOutput("git", ["status", "--porcelain", "--untracked-files=all"]) !== "")
    throw new RunnerError(3, "dirty-tree");
  const { appDataDirectory, workspaceDirectory } = isolatedEnvironmentDirectories(
    reportDirectory,
    true,
  );
  const report = preflightReportSchema.parse({
    schemaVersion: SCHEMA_VERSION,
    suite: "preflight",
    mode: "preflight",
    identity: currentIdentity(),
    startedAt,
    completedAt: new Date().toISOString(),
    workspaceInitialSha256: hashDirectory(workspaceDirectory, true),
    dedicatedAppDataEmpty: readdirSync(appDataDirectory).length === 0,
    result: "pass",
  });
  writeJsonExclusive(reportDirectory, "preflight.json", report);
  return report;
}

export interface RssSample {
  atMs: number;
  rssMiB: number;
}

export function median(values: number[]): number {
  if (values.length === 0) throw new RunnerError(3, "sampling-gap");
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0 ? (sorted[middle - 1]! + sorted[middle]!) / 2 : sorted[middle]!;
}

export function summarizeRssMedianDelta(samples: RssSample[], durationMs: number): number {
  const start = samples[0]?.atMs;
  if (start === undefined) throw new RunnerError(3, "sampling-gap");
  const twoHours = durationMs >= 7_200_000;
  const warmupMs = twoHours ? 300_000 : 60_000;
  const windowMs = twoHours ? 900_000 : 300_000;
  const first = samples
    .filter(
      (sample) => sample.atMs >= start + warmupMs && sample.atMs < start + warmupMs + windowMs,
    )
    .map((sample) => sample.rssMiB);
  const end = start + durationMs;
  const last = samples
    .filter((sample) => sample.atMs > end - windowMs && sample.atMs <= end)
    .map((sample) => sample.rssMiB);
  return Math.max(0, median(last) - median(first));
}

function sampleRss(pid: number): number {
  const command = commandOutput("ps", ["-p", String(pid), "-o", "rss="]);
  const kib = Number(command);
  if (!Number.isInteger(kib) || kib <= 0) throw new RunnerError(3, "process-exited");
  return kib / 1024;
}

function expectedAppExecutable(buildClass: BuildClass): string {
  return buildClass === "signed-packaged"
    ? join(configuredBundlePath(), "Contents/MacOS/saaa")
    : DEFAULT_DEVELOPMENT_EXECUTABLE;
}

function validatedAppPid(buildClass: BuildClass, suppliedPid?: number): number {
  const pid = suppliedPid ?? Number(process.env.SAAA_MVP2X_APP_PID);
  if (!Number.isInteger(pid) || pid <= 1) throw new RunnerError(3, "app-pid-invalid");
  const command = commandOutput("ps", ["-p", String(pid), "-o", "command="]);
  const executable = expectedAppExecutable(buildClass);
  if (!existsSync(executable) || lstatSync(executable).isSymbolicLink())
    throw new RunnerError(3, "app-pid-invalid");
  if (command !== executable && !command.startsWith(`${executable} `))
    throw new RunnerError(3, "app-pid-invalid");
  return pid;
}

async function promptAndValidateAppPid(
  reader: ReturnType<typeof createInterface>,
  spec: CaseSpec,
): Promise<void> {
  process.stdout.write(`\n[${spec.buildClass}] ${spec.caseId}\n${spec.instruction}\n`);
  const suppliedPid = Number((await reader.question("実行中の対象app PID: ")).trim());
  validatedAppPid(spec.buildClass, suppliedPid);
}

function workspaceIntegrityMismatch(workspaceDirectory: string, expectedSha256: string): number {
  try {
    const dirty =
      commandOutput(
        "git",
        ["status", "--porcelain", "--untracked-files=all"],
        workspaceDirectory,
      ) !== "";
    return dirty || hashDirectory(workspaceDirectory, true) !== expectedSha256 ? 1 : 0;
  } catch {
    return 1;
  }
}

async function collectRss(pid: number, durationMs: number): Promise<RssSample[]> {
  const samples: RssSample[] = [];
  const started = Date.now();
  let nextProgress = 300_000;
  while (true) {
    const elapsed = Date.now() - started;
    samples.push({ atMs: started + elapsed, rssMiB: sampleRss(pid) });
    if (elapsed >= durationMs) break;
    if (elapsed >= nextProgress) {
      process.stdout.write(`resource sampling: ${Math.floor(elapsed / 60_000)} minutes complete\n`);
      nextProgress += 300_000;
    }
    await new Promise((resolvePromise) =>
      setTimeout(resolvePromise, Math.min(5_000, durationMs - elapsed)),
    );
  }
  return samples;
}

async function promptCase(
  reader: ReturnType<typeof createInterface>,
  spec: CaseSpec,
  automatic: Map<string, number> | (() => Map<string, number>),
  startedAtOverride?: string,
  skipReady = false,
  introDisplayed = false,
): Promise<z.infer<typeof caseResultSchema>> {
  if (!skipReady) {
    if (!introDisplayed)
      process.stdout.write(`\n[${spec.buildClass}] ${spec.caseId}\n${spec.instruction}\n`);
    await reader.question("準備できたらEnter: ");
  }
  const startedAt = startedAtOverride ?? new Date().toISOString();
  const resultInput = (await reader.question("result (pass/fail/blocked): ")).trim();
  if (!RESULT_VALUES.includes(resultInput as Result))
    throw new RunnerError(3, "observation-invalid");
  let result = resultInput as Result;
  let reasonCode: ReasonCode | null = null;
  const observations: Array<z.infer<typeof observationSchema>> = [];
  if (result === "pass") {
    const automaticObservations = typeof automatic === "function" ? automatic() : automatic;
    for (const metric of spec.metrics) {
      const supplied = automaticObservations.get(metric.key);
      const raw =
        supplied === undefined
          ? (await reader.question(`${metric.description} [${metric.unit}]: `)).trim()
          : String(supplied);
      const value = Number(raw);
      try {
        assertMetric(metric, value);
        observations.push({ key: metric.key, value, unit: metric.unit });
      } catch (cause) {
        if (cause instanceof RunnerError && cause.code === "threshold-exceeded") {
          observations.push({ key: metric.key, value, unit: metric.unit });
          result = "fail";
          reasonCode = "threshold-exceeded";
          break;
        }
        throw cause;
      }
    }
  } else {
    const input = (await reader.question(`reason (${REASON_CODES.join("/")}): `)).trim();
    if (!REASON_CODES.includes(input as ReasonCode))
      throw new RunnerError(3, "observation-invalid");
    reasonCode = input as ReasonCode;
  }
  return caseResultSchema.parse({
    caseId: spec.caseId,
    buildClass: spec.buildClass,
    startedAt,
    completedAt: new Date().toISOString(),
    result,
    reasonCode,
    observations,
  });
}

export async function runVerify(
  reportDirectoryInput: string,
  suite: Suite,
  mode: Mode,
): Promise<SuiteReport> {
  const reportDirectory = requireDirectory(reportDirectoryInput, false);
  const preflight = preflightReportSchema.parse(readJson(join(reportDirectory, "preflight.json")));
  const workspaceDirectory = assertVerificationEnvironment(reportDirectory, preflight);
  if (!process.stdin.isTTY || !process.stdout.isTTY)
    throw new RunnerError(3, "interactive-operator-required");
  const specs = caseSpecs(suite, mode);
  const startedAt = new Date().toISOString();
  const reader = createInterface({ input: process.stdin, output: process.stdout });
  const cases: SuiteReport["cases"] = [];
  try {
    for (let index = 0; index < specs.length; index += 1) {
      const spec = specs[index]!;
      const automatic = new Map<string, number>();
      let recorded: z.infer<typeof caseResultSchema>;
      const inputActivitySoak = suite === "input-activity" && spec.caseId === "sampling-soak";
      if (mode === "soak-30m" || mode === "soak-2h" || inputActivitySoak) {
        const pid = validatedAppPid(spec.buildClass);
        process.stdout.write(`\n${spec.instruction}\n`);
        await reader.question(
          `${inputActivitySoak ? "Input Activity monitoring" : "Meeting"}を開始してからEnterするとRSS samplingを開始します: `,
        );
        const soakStartedAt = new Date().toISOString();
        const durationMs = mode === "soak-2h" ? 7_200_000 : 1_800_000;
        const samples = await collectRss(pid, durationMs);
        automatic.set("elapsedSeconds", durationMs / 1_000);
        automatic.set(
          inputActivitySoak ? "rssGrowthMiB" : "rssMedianDeltaMiB",
          summarizeRssMedianDelta(samples, durationMs),
        );
        recorded = await promptCase(reader, spec, automatic, soakStartedAt, true);
      } else {
        if (
          suite === "agent-run" &&
          workspaceIntegrityMismatch(workspaceDirectory, preflight.workspaceInitialSha256) !== 0
        )
          throw new RunnerError(3, "fixture-workspace-invalid");
        await promptAndValidateAppPid(reader, spec);
        recorded = await promptCase(
          reader,
          spec,
          suite === "agent-run"
            ? () =>
                new Map([
                  [
                    "workspaceDigestMismatchCount",
                    workspaceIntegrityMismatch(
                      workspaceDirectory,
                      preflight.workspaceInitialSha256,
                    ),
                  ],
                ])
            : automatic,
          undefined,
          false,
          true,
        );
      }
      cases.push(recorded);
      if (recorded.result !== "pass") {
        const timestamp = new Date().toISOString();
        for (const skipped of specs.slice(index + 1)) {
          cases.push(
            caseResultSchema.parse({
              caseId: skipped.caseId,
              buildClass: skipped.buildClass,
              startedAt: timestamp,
              completedAt: timestamp,
              result: "blocked",
              reasonCode: "operator-blocked",
              observations: [],
            }),
          );
        }
        break;
      }
    }
  } finally {
    reader.close();
  }
  if (commandOutput("git", ["status", "--porcelain", "--untracked-files=all"]) !== "")
    throw new RunnerError(3, "dirty-tree");
  assertCurrentIdentity(preflight.identity);
  const result: Result = cases.some((item) => item.result === "fail")
    ? "fail"
    : cases.some((item) => item.result === "blocked")
      ? "blocked"
      : "pass";
  const report = suiteReportSchema.parse({
    schemaVersion: SCHEMA_VERSION,
    suite,
    mode,
    identity: preflight.identity,
    startedAt,
    completedAt: new Date().toISOString(),
    cases,
    result,
  });
  validateSuiteCases(report);
  writeJsonExclusive(reportDirectory, reportFilename(suite, mode), report);
  return report;
}

export function validateSuiteCases(report: SuiteReport): void {
  const expected = caseSpecs(report.suite, report.mode)
    .map((item) => `${item.caseId}:${item.buildClass}`)
    .sort();
  const actual = report.cases.map((item) => `${item.caseId}:${item.buildClass}`).sort();
  if (JSON.stringify(expected) !== JSON.stringify(actual))
    throw new RunnerError(3, "case-matrix-invalid");
  for (const result of report.cases) {
    if (result.result !== "pass") continue;
    const spec = caseSpecs(report.suite, report.mode).find(
      (item) => item.caseId === result.caseId && item.buildClass === result.buildClass,
    )!;
    const metrics = new Map(result.observations.map((item) => [item.key, item]));
    if (metrics.size !== spec.metrics.length) throw new RunnerError(3, "observation-invalid");
    for (const metric of spec.metrics) {
      const observation = metrics.get(metric.key);
      if (!observation || observation.unit !== metric.unit)
        throw new RunnerError(3, "observation-invalid");
      assertMetric(metric, observation.value);
    }
  }
}

const EXPECTED_REPORTS: Array<[Suite, Mode]> = [
  ["input-activity", "manual"],
  ["agent-run", "manual"],
];

export function hashEvidenceReportSet(reportDirectory: string): string {
  const hash = createHash("sha256");
  const filenames = [
    "preflight.json",
    ...EXPECTED_REPORTS.map(([suite, mode]) => reportFilename(suite, mode)),
  ];
  for (const filename of filenames) {
    const path = join(reportDirectory, filename);
    if (!existsSync(path)) continue;
    readJson(path);
    const bytes = readFileSync(path);
    hash.update(`${filename}\0${bytes.length}\0`);
    hash.update(bytes);
  }
  return hash.digest("hex");
}

function scanReportDirectory(reportDirectory: string): number {
  const allowed = new Set([
    "preflight.json",
    ...EXPECTED_REPORTS.map(([suite, mode]) => reportFilename(suite, mode)),
  ]);
  let findings = 0;
  for (const name of readdirSync(reportDirectory)) {
    const path = join(reportDirectory, name);
    const info = lstatSync(path);
    if (
      !info.isFile() ||
      info.isSymbolicLink() ||
      info.nlink !== 1 ||
      info.size > MAX_REPORT_BYTES ||
      (info.mode & 0o777) !== 0o600
    )
      throw new RunnerError(3, "report-file-invalid");
    if (!allowed.has(name)) {
      findings += 1;
      if (FORBIDDEN_TEXT.test(readFileSync(path, "utf8"))) findings += 1;
      continue;
    }
    findings += forbiddenDataFindings(readJson(path));
  }
  return findings;
}

export function aggregateReports(reportDirectoryInput: string): AggregateReport {
  const startedAt = new Date().toISOString();
  const reportDirectory = requireDirectory(reportDirectoryInput, false);
  if (isWithin(ROOT, reportDirectory))
    throw new RunnerError(3, "report-directory-inside-repository");
  const preflight = preflightReportSchema.parse(readJson(join(reportDirectory, "preflight.json")));
  const expectedCaseCount = EXPECTED_REPORTS.reduce(
    (total, [suite, mode]) => total + caseSpecs(suite, mode).length,
    0,
  );
  let passedCaseCount = 0;
  let failedCaseCount = 0;
  let blockedCaseCount = 0;
  let missingCaseCount = 0;
  const forbiddenDataFindingCount = scanReportDirectory(reportDirectory);
  for (const [suite, mode] of EXPECTED_REPORTS) {
    const path = join(reportDirectory, reportFilename(suite, mode));
    if (!existsSync(path)) {
      missingCaseCount += caseSpecs(suite, mode).length;
      continue;
    }
    const raw = readJson(path);
    let report: SuiteReport;
    try {
      report = suiteReportSchema.parse(raw);
      if (
        report.suite !== suite ||
        report.mode !== mode ||
        !sameIdentity(report.identity, preflight.identity)
      )
        throw new RunnerError(3, "identity-mismatch");
      validateSuiteCases(report);
    } catch (cause) {
      if (cause instanceof RunnerError) throw cause;
      throw new RunnerError(3, "report-schema-invalid");
    }
    passedCaseCount += report.cases.filter((item) => item.result === "pass").length;
    failedCaseCount += report.cases.filter((item) => item.result === "fail").length;
    blockedCaseCount += report.cases.filter((item) => item.result === "blocked").length;
  }
  const accepted =
    passedCaseCount === expectedCaseCount &&
    failedCaseCount === 0 &&
    blockedCaseCount === 0 &&
    missingCaseCount === 0 &&
    forbiddenDataFindingCount === 0;
  const aggregate = aggregateReportSchema.parse({
    schemaVersion: SCHEMA_VERSION,
    suite: "aggregate",
    mode: "aggregate",
    identity: preflight.identity,
    startedAt,
    completedAt: new Date().toISOString(),
    expectedCaseCount,
    passedCaseCount,
    failedCaseCount,
    blockedCaseCount,
    missingCaseCount,
    forbiddenDataFindingCount,
    reportSetSha256: hashEvidenceReportSet(reportDirectory),
    result: accepted ? "accepted" : "not-accepted",
  });
  writeJsonExclusive(reportDirectory, "aggregate.json", aggregate);
  return aggregate;
}
