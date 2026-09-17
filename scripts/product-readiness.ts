import { existsSync } from "node:fs";
import { arch, platform } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import {
  ProductReadinessError,
  buildProductReadinessReport,
  parseProductReadinessArguments,
  writeProductReadinessReport,
  type AutomatedCheck,
} from "./product-readiness/report";

const root = fileURLToPath(new URL("..", import.meta.url));
const checks = [
  { id: "local", command: "bun", args: ["run", "check:local"] },
  { id: "rust-packages", command: "bun", args: ["run", "test:rust-packages"] },
  { id: "spec", command: "bun", args: ["run", "spec:check"] },
] as const;

function run(): void {
  const { reportDirectory } = parseProductReadinessArguments(process.argv.slice(2));
  if (existsSync(join(reportDirectory, "automated.json"))) {
    throw new ProductReadinessError("report-exists");
  }
  const results: AutomatedCheck[] = checks.map((check) => {
    const started = performance.now();
    const result = spawnSync(check.command, check.args, { cwd: root, stdio: "inherit" });
    return {
      id: check.id,
      elapsedMs: Math.round(performance.now() - started),
      result: result.status === 0 ? "passed" : "failed",
    };
  });
  const commit = spawnSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" });
  const status = spawnSync("git", ["status", "--porcelain"], { cwd: root, encoding: "utf8" });
  const report = buildProductReadinessReport({
    checks: results,
    commit: commit.status === 0 ? commit.stdout.trim() : "unknown",
    workingTree: status.status === 0 && status.stdout.length === 0 ? "clean" : "dirty",
    os: platform(),
    arch: arch(),
  });
  writeProductReadinessReport(reportDirectory, report);
  process.stdout.write(`${JSON.stringify(report)}\n`);
  if (report.automatedResult !== "passed") process.exitCode = 2;
}

try {
  run();
} catch (error) {
  const code = error instanceof ProductReadinessError ? error.code : "report-write-failed";
  process.stderr.write(`product-readiness: ${code}\n`);
  process.exitCode = 64;
}
