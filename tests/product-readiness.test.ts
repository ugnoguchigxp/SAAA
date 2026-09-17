import { afterAll, describe, expect, test } from "bun:test";
import { chmodSync, mkdtempSync, rmSync, statSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  ProductReadinessError,
  buildProductReadinessReport,
  parseProductReadinessArguments,
  writeProductReadinessReport,
} from "../scripts/product-readiness/report";

const temporaryDirectories: string[] = [];
afterAll(() => {
  for (const path of temporaryDirectories) rmSync(path, { recursive: true, force: true });
});

function temporaryDirectory(): string {
  const path = mkdtempSync(join(tmpdir(), "saaa-product-readiness-test-"));
  chmodSync(path, 0o700);
  temporaryDirectories.push(path);
  return path;
}

const passedChecks = [
  { id: "local", elapsedMs: 1, result: "passed" },
  { id: "rust-packages", elapsedMs: 1, result: "passed" },
  { id: "spec", elapsedMs: 1, result: "passed" },
] as const;

describe("product readiness evidence", () => {
  test("accepts only one absolute report directory", () => {
    expect(parseProductReadinessArguments(["--report-dir", "/tmp/evidence"])).toEqual({
      reportDirectory: "/tmp/evidence",
    });
    expect(() => parseProductReadinessArguments(["--report-dir", "relative"])).toThrow(
      ProductReadinessError,
    );
    expect(() => parseProductReadinessArguments(["--token", "secret"])).toThrow(
      ProductReadinessError,
    );
  });

  test("never presents a dirty working tree as release-ready", () => {
    const report = buildProductReadinessReport({
      checks: [...passedChecks],
      commit: "a".repeat(40),
      workingTree: "dirty",
      os: "darwin",
      arch: "arm64",
      generatedAt: "2026-09-17T00:00:00.000Z",
    });
    expect(report.automatedResult).toBe("passed");
    expect(report.releaseResult).toBe("blocked");
    expect(Object.keys(report.scenarioCoverage)).toEqual([
      "U01",
      "U02",
      "U03",
      "U04",
      "U05",
      "U06",
      "U07",
      "U08",
      "U09",
    ]);
  });

  test("blocks release evidence when the commit cannot be verified", () => {
    const report = buildProductReadinessReport({
      checks: [...passedChecks],
      commit: "unknown",
      workingTree: "clean",
      os: "darwin",
      arch: "arm64",
    });
    expect(report.automatedResult).toBe("passed");
    expect(report.releaseResult).toBe("blocked");
  });

  test("returns a bounded error without echoing rejected arguments", () => {
    const result = spawnSync("bun", ["scripts/product-readiness.ts", "--token", "secret-value"], {
      cwd: join(import.meta.dir, ".."),
      encoding: "utf8",
    });
    expect(result.status).toBe(64);
    expect(result.stdout).toBe("");
    expect(result.stderr).toBe("product-readiness: usage-error\n");
    expect(result.stderr).not.toContain("secret-value");
  });

  test("writes private evidence once and preserves the first result", () => {
    const directory = temporaryDirectory();
    const report = buildProductReadinessReport({
      checks: [...passedChecks],
      commit: "a".repeat(40),
      workingTree: "clean",
      os: "darwin",
      arch: "arm64",
      generatedAt: "2026-09-17T00:00:00.000Z",
    });
    writeProductReadinessReport(directory, report);
    expect(statSync(join(directory, "automated.json")).mode & 0o777).toBe(0o600);
    expect(() => writeProductReadinessReport(directory, report)).toThrow("report-exists");
  });
});
