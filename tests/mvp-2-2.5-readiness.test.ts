import { afterAll, describe, expect, test } from "bun:test";
import { chmodSync, linkSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  SCHEMA_VERSION,
  RunnerError,
  aggregateReports,
  assertMetric,
  caseResultSchema,
  caseSpecs,
  classifySigningDetails,
  directoriesOverlap,
  forbiddenDataFindings,
  hashDirectory,
  hashEvidenceReportSet,
  parseCliArguments,
  preflightReportSchema,
  suiteReportSchema,
  summarizeRssMedianDelta,
  validateSuiteCases,
  writeJsonExclusive,
  type Identity,
  type MetricSpec,
  type SuiteReport,
} from "../scripts/mvp-2-2.5-readiness";

const temporaryDirectories: string[] = [];

function temporaryDirectory(): string {
  const path = mkdtempSync(join(tmpdir(), "saaa-mvp2x-test-"));
  chmodSync(path, 0o700);
  temporaryDirectories.push(path);
  return path;
}

afterAll(() => {
  for (const path of temporaryDirectories) rmSync(path, { recursive: true, force: true });
});

const identity: Identity = {
  saaaCommit: "abcdef0",
  bundleSha256: "a".repeat(64),
  osVersion: "macOS 15.6 (24G84)",
  architecture: "arm64",
  signingClass: "developer-id-application",
  operator: "operator-01",
};

function passingValue(metric: MetricSpec): number {
  if (metric.exact !== undefined) return metric.exact;
  if (metric.min !== undefined) return metric.min;
  return 0;
}

function suiteReport(suite: SuiteReport["suite"], mode: SuiteReport["mode"], nextIdentity = identity): SuiteReport {
  const timestamp = "2026-08-29T00:00:00.000Z";
  return suiteReportSchema.parse({
    schemaVersion: SCHEMA_VERSION,
    suite,
    mode,
    identity: nextIdentity,
    startedAt: timestamp,
    completedAt: timestamp,
    cases: caseSpecs(suite, mode).map((spec) => ({
      caseId: spec.caseId,
      buildClass: spec.buildClass,
      startedAt: timestamp,
      completedAt: timestamp,
      result: "pass",
      reasonCode: null,
      observations: spec.metrics.map((metric) => ({ key: metric.key, value: passingValue(metric), unit: metric.unit })),
    })),
    result: "pass",
  });
}

function writePreflight(directory: string) {
  writeJsonExclusive(directory, "preflight.json", preflightReportSchema.parse({
    schemaVersion: SCHEMA_VERSION,
    suite: "preflight",
    mode: "preflight",
    identity,
    startedAt: "2026-08-29T00:00:00.000Z",
    completedAt: "2026-08-29T00:00:01.000Z",
    workspaceInitialSha256: "b".repeat(64),
    dedicatedAppDataEmpty: true,
    result: "pass",
  }));
}

describe("MVP 2 / 2.5 readiness CLI", () => {
  test("accepts only the documented command shapes", () => {
    expect(parseCliArguments(["preflight", "--report-dir", "/tmp/evidence"])).toEqual({ command: "preflight", reportDirectory: "/tmp/evidence" });
    expect(parseCliArguments(["verify", "--suite", "meeting", "--mode", "functional", "--report-dir", "/tmp/evidence"])).toEqual({ command: "verify", reportDirectory: "/tmp/evidence", suite: "meeting", mode: "functional" });
    expect(parseCliArguments(["report", "--report-dir", "/tmp/evidence"])).toEqual({ command: "report", reportDirectory: "/tmp/evidence" });
    expect(() => parseCliArguments(["verify", "--suite", "meeting", "--mode", "manual", "--report-dir", "/tmp/evidence"])).toThrow(RunnerError);
    expect(() => parseCliArguments(["preflight", "--report-dir", "relative"])).toThrow(RunnerError);
    expect(() => parseCliArguments(["preflight", "--report-dir", "/tmp/evidence", "--token", "secret"])).toThrow(RunnerError);
  });

  test("keeps CLI failures bounded and content-free", () => {
    const result = Bun.spawnSync(["bun", join(import.meta.dir, "..", "scripts", "mvp-2-2.5-readiness.ts")], {
      cwd: join(import.meta.dir, ".."),
      stdout: "pipe",
      stderr: "pipe",
    });
    expect(result.exitCode).toBe(64);
    expect(result.stdout.toString()).toBe("");
    expect(result.stderr.toString()).toBe("mvp2x: usage-error\n");
  });

  test("fixes the complete required case matrix", () => {
    const meetingFunctional = caseSpecs("meeting", "functional");
    expect(meetingFunctional.length).toBe(22);
    expect(meetingFunctional.findLastIndex((item) => item.buildClass === "development"))
      .toBeLessThan(meetingFunctional.findIndex((item) => item.buildClass === "signed-packaged"));
    expect(caseSpecs("meeting", "soak-30m").map((item) => item.caseId)).toEqual(["soak-30m"]);
    expect(caseSpecs("meeting", "soak-2h").map((item) => item.caseId)).toEqual(["soak-2h"]);
    expect(caseSpecs("input-activity", "manual").length).toBe(23);
    expect(caseSpecs("agent-run", "manual").length).toBe(7);
    expect(caseSpecs("input-activity", "manual").find((item) => item.caseId === "sampling-soak")?.metrics.map((metric) => metric.automatic)).toEqual(["elapsed-seconds", undefined, "rss-median-delta"]);
    expect(caseSpecs("agent-run", "manual").every((item) => item.metrics.some((metric) => metric.automatic === "workspace-integrity"))).toBeTrue();
  });

  test("routes readiness runs to isolated application data", async () => {
    const runtime = await Bun.file(join(import.meta.dir, "..", "src-tauri", "src", "lib.rs")).text();
    expect(runtime).toContain('env::var_os("SAAA_MVP2X_APP_DATA_DIR")');
    expect(runtime).toContain("validate_readiness_data_directory");
    expect(runtime).toContain("must not use normal application data");
    expect(runtime).toContain('state.data_directory.join("diagnostics")');
    expect(runtime).toContain('state.data_directory.join("backups")');
    const runner = await Bun.file(join(import.meta.dir, "..", "scripts", "mvp-2-2.5-readiness.ts")).text();
    expect(runner).toContain("promptAndValidateAppPid");
    expect(runner).toContain("DEFAULT_DEVELOPMENT_EXECUTABLE");
  });

  test("rejects nested evidence, app-data, and workspace directories", () => {
    expect(directoriesOverlap("/tmp/evidence", "/tmp/evidence/app-data")).toBeTrue();
    expect(directoriesOverlap("/tmp/fixture/repository", "/tmp/fixture")).toBeTrue();
    expect(directoriesOverlap("/tmp/evidence", "/tmp/workspace")).toBeFalse();
  });
});

describe("strict evidence contracts", () => {
  test("rejects unknown fields and checkbox-only passes", () => {
    const base = {
      caseId: "permission-grant",
      buildClass: "development",
      startedAt: "2026-08-29T00:00:00.000Z",
      completedAt: "2026-08-29T00:00:01.000Z",
      result: "pass",
      reasonCode: null,
      observations: [],
    };
    expect(caseResultSchema.safeParse(base).success).toBeFalse();
    expect(caseResultSchema.safeParse({ ...base, result: "blocked", reasonCode: null }).success).toBeFalse();
    expect(caseResultSchema.safeParse({ ...base, result: "blocked", reasonCode: "operator-blocked", observations: [], note: "free text" }).success).toBeFalse();
  });

  test("enforces exact, minimum, and maximum thresholds", () => {
    expect(() => assertMetric({ key: "queue", unit: "count", description: "queue", max: 2 }, 2)).not.toThrow();
    expect(() => assertMetric({ key: "queue", unit: "count", description: "queue", max: 2 }, 3)).toThrow("threshold-exceeded");
    expect(() => assertMetric({ key: "samples", unit: "count", description: "samples", min: 890 }, 889)).toThrow("threshold-exceeded");
    expect(() => assertMetric({ key: "terminal", unit: "count", description: "terminal", exact: 1 }, 2)).toThrow("threshold-exceeded");
  });

  test("rejects content, secrets, network locations, and local paths", () => {
    expect(forbiddenDataFindings({ result: "pass", permissionPromptCount: 0 })).toBe(0);
    expect(forbiddenDataFindings({ prompt: "summarize" })).toBeGreaterThan(0);
    expect(forbiddenDataFindings({ value: "/Users/example/private" })).toBeGreaterThan(0);
    expect(forbiddenDataFindings({ value: "https://internal.example" })).toBeGreaterThan(0);
    expect(forbiddenDataFindings({ value: "192.168.0.65" })).toBeGreaterThan(0);
    expect(forbiddenDataFindings({ value: "172.20.10.4" })).toBeGreaterThan(0);
    expect(forbiddenDataFindings({ value: "ssh operator@gnosis" })).toBeGreaterThan(0);
  });

  test("requires an Apple certificate chain, Team ID, and signing extension", () => {
    const appleRootFingerprint = "B0:B1:73:0E:CB:C7:FF:45:05:14:2C:49:F1:29:5E:6E:DA:6B:CA:ED:7E:2C:68:C5:BE:91:B5:A1:10:01:F0:24";
    const developerDetails = [
      "Authority=Developer ID Application: Example (ABCDE12345)",
      "Authority=Developer ID Certification Authority",
      "Authority=Apple Root CA",
      "TeamIdentifier=ABCDE12345",
    ].join("\n");
    const developmentDetails = [
      "Authority=Apple Development: Example (ABCDE12345)",
      "Authority=Apple Worldwide Developer Relations Certification Authority",
      "Authority=Apple Root CA",
      "TeamIdentifier=ABCDE12345",
    ].join("\n");
    expect(classifySigningDetails(developerDetails, "1.2.840.113635.100.6.1.13", appleRootFingerprint)).toBe("developer-id-application");
    expect(classifySigningDetails(developmentDetails, "1.2.840.113635.100.6.1.12", appleRootFingerprint)).toBe("apple-development");
    expect(() => classifySigningDetails(developerDetails, "1.2.840.113635.100.6.1.13", "AA".repeat(32))).toThrow("signing-class-invalid");
    expect(() => classifySigningDetails('Authority=Developer ID Application: Fake\nTeamIdentifier=ABCDE12345', "1.2.840.113635.100.6.1.13", appleRootFingerprint)).toThrow("signing-class-invalid");
    expect(() => classifySigningDetails(`${developerDetails}\nSignature=adhoc`, "1.2.840.113635.100.6.1.13", appleRootFingerprint)).toThrow("signature-invalid");
  });

  test("writes atomically with mode 0600 and refuses overwrite", () => {
    const directory = temporaryDirectory();
    writeJsonExclusive(directory, "safe.json", { result: "pass" });
    expect(Bun.file(join(directory, "safe.json")).text()).resolves.toContain('"result": "pass"');
    expect(() => writeJsonExclusive(directory, "safe.json", { result: "pass" })).toThrow("report-overwrite-refused");
    expect(() => writeJsonExclusive(directory, "unsafe.json", { workspacePath: "/tmp/x" })).toThrow("redaction-failed");
  });

  test("validates exact cases, observations, and units", () => {
    const report = suiteReport("agent-run", "manual");
    expect(() => validateSuiteCases(report)).not.toThrow();
    const missing = { ...report, cases: report.cases.slice(1) };
    expect(() => validateSuiteCases(suiteReportSchema.parse({ ...missing, result: "pass" }))).toThrow("case-matrix-invalid");
    const changed = structuredClone(report);
    changed.cases[0]!.observations[0]!.unit = "seconds";
    expect(() => validateSuiteCases(changed)).toThrow("observation-invalid");
  });
});

describe("resource and aggregate evaluation", () => {
  test("uses bounded median windows instead of a single maximum", () => {
    const start = 1_000_000;
    const samples = [];
    for (let second = 0; second <= 1_800; second += 5) {
      const inLastWindow = second > 1_500;
      samples.push({ atMs: start + second * 1_000, rssMiB: inLastWindow ? 132 : 100 });
    }
    expect(summarizeRssMedianDelta(samples, 1_800_000)).toBe(32);
  });

  test("hashes content deterministically without embedding its path", () => {
    const directory = temporaryDirectory();
    mkdirSync(join(directory, "nested"), { mode: 0o700 });
    writeFileSync(join(directory, "nested", "fixture.txt"), "fixture", { mode: 0o600 });
    expect(hashDirectory(directory)).toMatch(/^[0-9a-f]{64}$/);
    expect(hashDirectory(directory)).toBe(hashDirectory(directory));
  });

  test("rejects hard-linked bundle or fixture content", () => {
    const directory = temporaryDirectory();
    writeFileSync(join(directory, "source"), "fixture", { mode: 0o600 });
    linkSync(join(directory, "source"), join(directory, "alias"));
    expect(() => hashDirectory(directory)).toThrow("environment-invalid");
  });

  test("accepts only a complete, identity-consistent report set", () => {
    const directory = temporaryDirectory();
    writePreflight(directory);
    const reports: Array<[SuiteReport["suite"], SuiteReport["mode"]]> = [
      ["meeting", "functional"],
      ["meeting", "soak-30m"],
      ["meeting", "soak-2h"],
      ["input-activity", "manual"],
      ["agent-run", "manual"],
    ];
    for (const [suite, mode] of reports) writeJsonExclusive(directory, `${suite}-${mode}.json`, suiteReport(suite, mode));
    const aggregate = aggregateReports(directory);
    expect(aggregate.result).toBe("accepted");
    expect(aggregate.passedCaseCount).toBe(aggregate.expectedCaseCount);
    expect(aggregate.forbiddenDataFindingCount).toBe(0);
    expect(aggregate.reportSetSha256).toBe(hashEvidenceReportSet(directory));
  });

  test("returns not-accepted when any report is missing", () => {
    const directory = temporaryDirectory();
    writePreflight(directory);
    const aggregate = aggregateReports(directory);
    expect(aggregate.result).toBe("not-accepted");
    expect(aggregate.missingCaseCount).toBeGreaterThan(0);
  });

  test("scans and rejects every unexpected report-directory file", () => {
    const directory = temporaryDirectory();
    writePreflight(directory);
    writeFileSync(join(directory, "operator-notes.json"), '{"prompt":"private"}\n', { mode: 0o600 });
    const aggregate = aggregateReports(directory);
    expect(aggregate.result).toBe("not-accepted");
    expect(aggregate.forbiddenDataFindingCount).toBeGreaterThan(0);
  });

  test("rejects digest or signing identity mismatches", () => {
    const directory = temporaryDirectory();
    writePreflight(directory);
    const mismatched = { ...identity, bundleSha256: "c".repeat(64) };
    writeJsonExclusive(directory, "meeting-functional.json", suiteReport("meeting", "functional", mismatched));
    expect(() => aggregateReports(directory)).toThrow("identity-mismatch");
  });
});
