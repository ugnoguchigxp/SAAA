import { afterEach, describe, expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, chmodSync, linkSync, statSync, realpathSync, symlinkSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  COMPILED_LARM_CONTRACT_COMMIT,
  ForbiddenDataScanner,
  REPORT_FILENAMES,
  REPORT_FORMAT,
  RunnerError,
  aggregateReports,
  appChildEnvironment,
  atomicWriteReport,
  buildEnvironment,
  canonicalBundleDigest,
  emptyReport,
  evaluateReport,
  mergeReports,
  loadManifest,
  parseCliArguments,
  rustChildEnvironment,
  validateHeaderCredential,
  validateNumericLoopbackOrigin,
  validateReport,
  validateReportDirectory,
  validateSoakObservation,
  type ReadinessReport,
  type ReportMode,
} from "../scripts/larm-readiness.ts";

const temporaryDirectories: string[] = [];

function temporaryDirectory(): string {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "saaa-larm-readiness-test-")));
  temporaryDirectories.push(directory);
  return directory;
}

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) rmSync(directory, { recursive: true, force: true });
});

function identity() {
  return {
    saaaCommit: "1234567",
    artifactSha256: "a".repeat(64),
    manifestSha256: "b".repeat(64),
    larmContractCommit: COMPILED_LARM_CONTRACT_COMMIT,
    deploymentRevision: "revision-1",
  };
}

function report(mode: Exclude<ReportMode, "aggregate">, result: ReadinessReport["result"] = "passed"): ReadinessReport {
  const value = emptyReport(identity(), mode, result, result === "passed" ? [] : ["gate-missing"]);
  if (mode === "functional") {
    value.leaseSummary.effectiveTtlSecondsMin = 60;
    value.leaseSummary.effectiveTtlSecondsMax = 300;
  }
  return value;
}

describe("LARM readiness CLI", () => {
  test("accepts only the fixed subcommands and argument shapes", () => {
    expect(parseCliArguments(["preflight", "--report-dir", "/tmp/report"])).toEqual({ command: "preflight", reportDirectory: "/tmp/report" });
    expect(parseCliArguments(["soak", "--duration", "30m", "--report-dir", "/tmp/report"])).toEqual({ command: "soak", reportDirectory: "/tmp/report", duration: "30m" });
    for (const arguments_ of [
      [],
      ["unknown", "--report-dir", "/tmp/report"],
      ["preflight"],
      ["preflight", "--report-dir", "relative"],
      ["preflight", "--report-dir", "/tmp/a", "--report-dir", "/tmp/b"],
      ["canary", "--report-dir", "/tmp/a", "--duration", "30m"],
      ["soak", "--report-dir", "/tmp/a"],
      ["soak", "--report-dir", "/tmp/a", "--duration", "10m"],
      ["report", "--report-dir", "/tmp/a", "--token", "secret"],
    ]) {
      expect(() => parseCliArguments(arguments_)).toThrow(RunnerError);
    }
  });

  test("accepts only exact HTTP numeric-loopback origins", () => {
    expect(validateNumericLoopbackOrigin("http://127.0.0.1:9810")).toBe("http://127.0.0.1:9810");
    expect(validateNumericLoopbackOrigin("http://[::1]:9810")).toBe("http://[::1]:9810");
    for (const value of [
      "http://localhost:9810",
      "http://127.0.0.1",
      "https://127.0.0.1:9810",
      "http://127.0.0.1:9810/",
      "http://127.0.0.1:9810/v1",
      "http://user@127.0.0.1:9810",
      "http://127.0.0.1:9810?x=1",
      "http://10.0.0.42:9810",
    ]) {
      expect(() => validateNumericLoopbackOrigin(value)).toThrow(RunnerError);
    }
  });

  test("credential values are bounded and header-safe", () => {
    expect(validateHeaderCredential("visible-token")).toBe("visible-token");
    for (const value of [undefined, "", "has space", "line\nbreak", "é", "x".repeat(4_097)]) {
      expect(() => validateHeaderCredential(value)).toThrow(RunnerError);
    }
  });

  test("usage errors keep the one-line stdout contract", async () => {
    const process_ = Bun.spawn(["bun", "scripts/larm-readiness.ts", "soak", "--report-dir", "/tmp/report", "--duration", "10m"], {
      cwd: join(import.meta.dir, ".."),
      env: { PATH: process.env.PATH ?? "/usr/bin:/bin", HOME: process.env.HOME ?? tmpdir() },
      stdout: "pipe",
      stderr: "pipe",
    });
    const [exitCode, stdout, stderr] = await Promise.all([
      process_.exited,
      new Response(process_.stdout).text(),
      new Response(process_.stderr).text(),
    ]);
    expect(exitCode).toBe(64);
    expect(stderr).toBe("usage-error\n");
    expect(stdout).toBe(`${JSON.stringify({ format: REPORT_FORMAT, mode: "preflight", result: "failed" })}\n`);
  });
});

describe("readiness report contract", () => {
  test("runner contract pin matches the production Rust adapter", () => {
    const source = readFileSync(join(import.meta.dir, "../src-tauri/src/providers/larm/mod.rs"), "utf8");
    expect(source).toContain(`pub(crate) const CONTRACT_COMMIT: &str = "${COMPILED_LARM_CONTRACT_COMMIT}";`);
  });

  test("rejects unknown fields, oversized values, invalid TTL, and duplicate failure codes", () => {
    expect(validateReport(report("preflight"))).toBeTruthy();
    expect(validateReport({
      ...report("preflight"),
      startedAt: "2026-08-29T00:00:00.123456789Z",
      finishedAt: "2026-08-29T00:00:00.123456789Z",
    })).toBeTruthy();
    expect(() => validateReport({
      ...report("preflight"),
      startedAt: "2026-02-30T00:00:00Z",
    })).toThrow(RunnerError);
    expect(() => validateReport({
      ...report("preflight"),
      startedAt: "2026-08-29T00:00:00.123456789Z",
      finishedAt: "2026-08-29T00:00:00.123456788Z",
    })).toThrow(RunnerError);
    expect(() => validateReport({ ...report("preflight"), extra: true })).toThrow(RunnerError);
    expect(() => validateReport({ ...report("preflight"), saaaCommit: "ABC1234" })).toThrow(RunnerError);
    const ttl = report("preflight");
    ttl.leaseSummary.effectiveTtlSecondsMin = 60;
    expect(() => validateReport(ttl)).toThrow(RunnerError);
    const duplicate = report("preflight", "failed");
    duplicate.failureCodes = ["internal", "internal"];
    expect(() => validateReport(duplicate)).toThrow(RunnerError);
    const blockedWithoutTtl = emptyReport(identity(), "functional", "blocked", ["gate-missing"]);
    expect(validateReport(blockedWithoutTtl).leaseSummary.effectiveTtlSecondsMax).toBe(0);
    blockedWithoutTtl.leaseSummary.effectiveTtlSecondsMin = 60;
    expect(() => validateReport(blockedWithoutTtl)).toThrow(RunnerError);
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    expect(() => validateReport(cyclic)).toThrow(RunnerError);
    expect(() => validateReport(undefined)).toThrow(RunnerError);
    expect(() => validateReport({ ...report("preflight"), invalid: 1n })).toThrow(RunnerError);
    const missingFailure = report("preflight", "failed");
    missingFailure.failureCodes = [];
    expect(() => validateReport(missingFailure)).toThrow(RunnerError);
    const inconsistentRedaction = report("preflight", "failed");
    inconsistentRedaction.failureCodes = ["redaction-failed"];
    expect(() => validateReport(inconsistentRedaction)).toThrow(RunnerError);
  });

  test("evaluator enforces exact functional counts and leak invariants", () => {
    const functional = report("functional");
    Object.assign(functional.scenarioCounts, {
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
    });
    Object.assign(functional.resultCounts, {
      completed: 14,
      cancelled: 2,
      expectedFailures: 3,
      explicitProviderFallbacks: 2,
    });
    functional.resourceSummary.maxActiveAllocations = 1;
    functional.leaseSummary.renewalsAttempted = 1;
    functional.leaseSummary.renewalsSucceeded = 1;
    expect(evaluateReport(functional).result).toBe("passed");
    functional.resultCounts.implicitFallbacks = 1;
    expect(evaluateReport(functional).result).toBe("failed");
  });

  test("merge preserves ownership and strongest result", () => {
    const rust = report("functional");
    rust.scenarioCounts.normalTurns = 2;
    rust.resultCounts.completed = 2;
    rust.resourceSummary.maxActiveAllocations = 1;
    rust.leaseSummary.renewalsAttempted = 1;
    rust.leaseSummary.renewalsSucceeded = 1;
    const local = emptyReport(identity(), "functional", "blocked", ["gate-missing"]);
    local.scenarioCounts.normalTurns = 3;
    local.resultCounts.completed = 3;
    const merged = mergeReports(rust, local);
    expect(merged.result).toBe("blocked");
    expect(merged.resourceSummary.maxActiveAllocations).toBe(1);
    expect(merged.leaseSummary.renewalsSucceeded).toBe(1);

    const invalidOwner = emptyReport(identity(), "functional");
    invalidOwner.resourceSummary.maxActiveAllocations = 1;
    expect(() => mergeReports(rust, invalidOwner)).toThrow(RunnerError);
  });

  test("aggregate requires all four ordered modes and matching identity", () => {
    const preflight = report("preflight", "failed");
    preflight.failureCodes = ["internal"];
    const functional = report("functional", "blocked");
    const soak30 = report("soak-30m", "blocked");
    const soak2 = report("soak-2h", "blocked");
    const aggregate = aggregateReports([preflight, functional, soak30, soak2]);
    expect(aggregate.mode).toBe("aggregate");
    expect(aggregate.result).toBe("failed");
    expect(aggregate.failureCodes).toEqual(["internal", "gate-missing"]);
  });

  test("soak evaluator enforces duration, sampling, RSS, and workload thresholds", () => {
    const soak = report("soak-30m");
    soak.scenarioCounts.normalTurns = 20;
    soak.scenarioCounts.cancellations = 5;
    soak.resultCounts.completed = 20;
    soak.resultCounts.cancelled = 5;
    soak.timingSummary.elapsedMs = 1_800_000;
    soak.timingSummary.sampleIntervalSeconds = 5;
    soak.timingSummary.rssMaxSamplingGapSeconds = 5;
    soak.timingSummary.metricsMaxSamplingGapSeconds = 5;
    soak.resourceSummary.maxActiveAllocations = 1;
    soak.resourceSummary.rssRangeMiB = 64;
    expect(evaluateReport(soak).result).toBe("passed");
    soak.timingSummary.rssMaxSamplingGapSeconds = 16;
    const failed = evaluateReport(soak);
    expect(failed.result).toBe("failed");
    expect(failed.failureCodes).toContain("sampling-gap");

    const soak2 = report("soak-2h");
    soak2.scenarioCounts.normalTurns = 60;
    soak2.scenarioCounts.cancellations = 10;
    soak2.scenarioCounts.larmRestarts = 1;
    soak2.scenarioCounts.saaaRestarts = 1;
    soak2.resultCounts.completed = 60;
    soak2.resultCounts.cancelled = 10;
    soak2.timingSummary.elapsedMs = 7_200_000;
    soak2.timingSummary.sampleIntervalSeconds = 5;
    soak2.timingSummary.rssMaxSamplingGapSeconds = 15;
    soak2.timingSummary.metricsMaxSamplingGapSeconds = 15;
    soak2.resourceSummary.maxActiveAllocations = 1;
    soak2.resourceSummary.rssRangeMiB = 64;
    soak2.resourceSummary.rssPrevious30mMedianMiB = 100;
    soak2.resourceSummary.rssLast30mMedianMiB = 116;
    expect(evaluateReport(soak2).failureCodes).toContain("sampling-gap");
    soak2.timingSummary.plannedLarmRestartGapSeconds = 1;
    expect(evaluateReport(soak2).result).toBe("passed");
  });

  test("soak database observation accepts exact cancelled terminal state only", () => {
    const observation = {
      runs: [
        { id: "run_completed", conversation_id: "conversation_1", provider_id: "larm-local", status: "completed" as const },
        { id: "run_cancelled", conversation_id: "conversation_2", provider_id: "larm-local", status: "cancelled" as const },
      ],
      sessions: [
        {
          id: "session_completed", runtime_run_id: "run_completed", provider_id: "larm-local", provider_kind: "larm" as const,
          allocation_id: "allocation_1", selected_runtime_id: "qwen-general", request_id: "request_1", fallback_used: 0 as const,
          route_id: "llm-default", selection_reason: "primary" as const, output_started: 1 as const, failure_kind: null,
          release_status: "released" as const, status: "completed" as const,
        },
        {
          id: "session_cancelled", runtime_run_id: "run_cancelled", provider_id: "larm-local", provider_kind: "larm" as const,
          allocation_id: "allocation_2", selected_runtime_id: "qwen-general", request_id: null, fallback_used: 0 as const,
          route_id: "llm-default", selection_reason: "primary" as const, output_started: 1 as const, failure_kind: "cancelled" as const,
          release_status: "released" as const, status: "cancelled" as const,
        },
      ],
    };
    expect(validateSoakObservation(observation, "larm-local")).toEqual({ completed: 1, cancelled: 1 });
    observation.sessions[1]!.failure_kind = null as never;
    expect(() => validateSoakObservation(observation, "larm-local")).toThrow(RunnerError);
  });
});

describe("readiness security boundaries", () => {
  test("scanner detects exact data across chunk boundaries and auth patterns", () => {
    const scanner = new ForbiddenDataScanner(["private-value"]);
    expect(scanner.scan(Buffer.from("x".repeat(4_095) + "private-"))).toBe(false);
    expect(scanner.scan(Buffer.from("value"))).toBe(true);
    const authorization = new ForbiddenDataScanner([]);
    expect(authorization.scan(Buffer.from("Authorization: Bearer anything"))).toBe(true);
    const identifier = new ForbiddenDataScanner([]);
    expect(identifier.scan(Buffer.from('{"allocationId":"alloc_secret"}'))).toBe(true);
    const content = new ForbiddenDataScanner(["Reply with exactly: READY", "http://127.0.0.1:9810"]);
    expect(content.scan(Buffer.from("Reply with exactly: READY"))).toBe(true);
  });

  test("child environment allowlists exclude unrelated credentials and proxies", () => {
    const environment = {
      PATH: "/bin",
      HOME: "/tmp/home",
      LARM_API_TOKEN: "larm",
      SAAA_LARM_CANARY: "1",
      SAAA_LARM_ENABLED: "1",
      SAAA_LARM_CANARY_BASE_URL: "http://127.0.0.1:9810",
      SAAA_LARM_DEPLOYED_COMMIT: "1234567",
      SAAA_LARM_DEPLOYMENT_REVISION: "r1",
      SAAA_PROVIDER_LOCAL_OPENAI_COMPATIBLE_API_KEY: "rollback",
      OPENAI_API_KEY: "unrelated",
      HTTPS_PROXY: "proxy",
      AWS_SECRET_ACCESS_KEY: "secret",
      RANDOM_SETTING: "no",
      GIT_DIR: "/tmp/attacker-controlled-git-dir",
      CARGO_TARGET_DIR: "/tmp/stale-target",
      RUSTC_WRAPPER: "/tmp/wrapper",
    };
    expect(buildEnvironment(environment)).not.toHaveProperty("OPENAI_API_KEY");
    expect(buildEnvironment(environment)).not.toHaveProperty("HTTPS_PROXY");
    expect(buildEnvironment(environment)).not.toHaveProperty("AWS_SECRET_ACCESS_KEY");
    expect(buildEnvironment(environment)).not.toHaveProperty("RANDOM_SETTING");
    expect(buildEnvironment(environment)).not.toHaveProperty("GIT_DIR");
    expect(buildEnvironment(environment)).not.toHaveProperty("CARGO_TARGET_DIR");
    expect(buildEnvironment(environment)).not.toHaveProperty("RUSTC_WRAPPER");
    const rust = rustChildEnvironment(environment, {
      resultFile: "/tmp/result",
      artifactSha256: "a".repeat(64),
      manifestSha256: "b".repeat(64),
      metricsScope: "client-scoped",
    });
    expect(rust.LARM_API_TOKEN).toBe("larm");
    expect(rust.SAAA_LARM_CANARY_METRICS_SCOPE).toBe("client-scoped");
    expect(rust).not.toHaveProperty("SAAA_PROVIDER_LOCAL_OPENAI_COMPATIBLE_API_KEY");
    expect(rust).not.toHaveProperty("RANDOM_SETTING");
    const app = appChildEnvironment(environment, { enabled: false, markerId: "canary-" + "a".repeat(32), dataDirectory: "/tmp/data" });
    expect(app.SAAA_LARM_ENABLED).toBe("0");
    expect(app.SAAA_PROVIDER_LOCAL_OPENAI_COMPATIBLE_API_KEY).toBe("rollback");
    expect(app).not.toHaveProperty("OPENAI_API_KEY");
  });

  test("canonical bundle digest is deterministic and rejects hard links", async () => {
    const bundle = join(temporaryDirectory(), "SAAA.app");
    mkdirSync(join(bundle, "Contents/MacOS"), { recursive: true });
    writeFileSync(join(bundle, "Contents/MacOS/saaa"), "executable");
    chmodSync(join(bundle, "Contents/MacOS/saaa"), 0o700);
    mkdirSync(join(bundle, "Contents/Resources"));
    writeFileSync(join(bundle, "Contents/Resources/a.txt"), "a");
    const first = await canonicalBundleDigest(bundle);
    const second = await canonicalBundleDigest(bundle);
    expect(first).toMatch(/^[0-9a-f]{64}$/);
    expect(second).toBe(first);
    linkSync(join(bundle, "Contents/Resources/a.txt"), join(bundle, "Contents/Resources/b.txt"));
    await expect(canonicalBundleDigest(bundle)).rejects.toBeInstanceOf(RunnerError);
    const missing = join(temporaryDirectory(), "missing.app");
    await expect(canonicalBundleDigest(missing)).rejects.toMatchObject({ errorCode: "artifact-mismatch" });
  });

  test("reports are mode 0600, atomic, and never overwritten", () => {
    const directory = temporaryDirectory();
    chmodSync(directory, 0o700);
    const filename = join(directory, REPORT_FILENAMES.preflight);
    atomicWriteReport(filename, report("preflight"));
    expect(statSync(filename).mode & 0o777).toBe(0o600);
    expect(() => atomicWriteReport(filename, report("preflight"))).toThrow(RunnerError);
    expect(validateReportDirectory(directory, "canary")).toBe(directory);
    expect(() => validateReportDirectory(join(directory, "missing"), "preflight")).toThrow(RunnerError);
  });

  test("manifest parser enforces strict schema, permissions, and data lifecycle", () => {
    const operator = temporaryDirectory();
    const dataDirectory = join(operator, "data");
    mkdirSync(dataDirectory, { mode: 0o700 });
    const filename = join(operator, "manifest.json");
    const manifest = {
      format: "saaa-larm-canary-manifest-v1",
      saaaCommit: "1234567",
      larmContractCommit: COMPILED_LARM_CONTRACT_COMMIT,
      deploymentRevision: "revision-1",
      dataDirectory,
      metricsScope: "exclusive-window",
      larmProvider: {
        baseUrl: "http://127.0.0.1:9810",
        allocationTtlSeconds: 300,
        allocationStartupTimeoutSeconds: 300,
        allowFallbackByDefault: false,
        deploymentPolicy: "existing-only",
      },
      rollbackProvider: {
        id: "local-openai-compatible",
        location: "local",
        endpoint: "http://127.0.0.1:8080/v1",
        model: "rollback-model",
        credentialEnv: "SAAA_PROVIDER_LOCAL_OPENAI_COMPATIBLE_API_KEY",
        credentialRequired: true,
      },
    } as const;
    writeFileSync(filename, JSON.stringify(manifest), { mode: 0o600 });
    const loaded = loadManifest(filename, "preflight");
    expect(loaded.manifest.dataDirectory).toBe(dataDirectory);
    expect(loaded.sha256).toMatch(/^[0-9a-f]{64}$/);
    writeFileSync(join(dataDirectory, "saaa.sqlite3"), "fixture");
    expect(loadManifest(filename, "later").manifest.format).toBe("saaa-larm-canary-manifest-v1");

    const unknownFilename = join(operator, "unknown.json");
    writeFileSync(unknownFilename, JSON.stringify({ ...manifest, unknown: true }), { mode: 0o600 });
    expect(() => loadManifest(unknownFilename, "later")).toThrow(RunnerError);
    const linkFilename = join(operator, "link.json");
    symlinkSync(filename, linkFilename);
    expect(() => loadManifest(linkFilename, "later")).toThrow(RunnerError);
    const malformedUtf8 = join(operator, "malformed-utf8.json");
    writeFileSync(malformedUtf8, Buffer.from([0x7b, 0x22, 0xff, 0x22, 0x7d]), { mode: 0o600 });
    expect(() => loadManifest(malformedUtf8, "later")).toThrow(RunnerError);
  });

  test("missing G1 blocks before any loopback request", async () => {
    let requests = 0;
    const server = Bun.serve({
      port: 0,
      fetch: () => {
        requests += 1;
        return new Response("unexpected", { status: 500 });
      },
    });
    const directory = temporaryDirectory();
    chmodSync(directory, 0o700);
    try {
      const process_ = Bun.spawn([
        "bun",
        "scripts/larm-readiness.ts",
        "preflight",
        "--report-dir",
        directory,
      ], {
        cwd: join(import.meta.dir, ".."),
        env: {
          PATH: process.env.PATH ?? "/usr/bin:/bin",
          HOME: process.env.HOME ?? tmpdir(),
          SAAA_LARM_CANARY: "1",
          SAAA_LARM_ENABLED: "1",
          LARM_API_TOKEN: "fixture-token",
          SAAA_LARM_CANARY_BASE_URL: `http://127.0.0.1:${server.port}`,
        },
        stdout: "pipe",
        stderr: "pipe",
      });
      const [exitCode, stdout, stderr] = await Promise.all([
        process_.exited,
        new Response(process_.stdout).text(),
        new Response(process_.stderr).text(),
      ]);
      expect(exitCode).toBe(3);
      expect(stderr).toBe("gate-missing\n");
      expect(JSON.parse(stdout)).toEqual({ format: REPORT_FORMAT, mode: "preflight", result: "blocked" });
      expect(requests).toBe(0);
    } finally {
      server.stop(true);
    }
  });
});
