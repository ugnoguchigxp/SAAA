import { mkdirSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const COVERAGE = join(ROOT, "coverage");

function run(command: string, args: string[], extraEnv: Record<string, string> = {}) {
  const result = spawnSync(command, args, {
    cwd: ROOT,
    env: { ...process.env, ...extraEnv },
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  return result.status ?? 1;
}

mkdirSync(join(COVERAGE, "frontend"), { recursive: true });
const frontend = run("bun", [
  "test",
  "tests",
  "--coverage",
  "--coverage-reporter=lcov",
  `--coverage-dir=${join(COVERAGE, "frontend")}`,
]);
if (frontend !== 0) process.exit(frontend);

const llvmCov = spawnSync("cargo", ["llvm-cov", "--version"], { cwd: ROOT, encoding: "utf8" });
if (llvmCov.status !== 0) {
  console.log("cargo-llvm-cov is not installed; skipping the Rust HTML report.");
  console.log("Install it with: cargo install cargo-llvm-cov --locked");
  console.log(`Frontend LCOV: ${join(COVERAGE, "frontend")}`);
  process.exit(0);
}

mkdirSync(join(COVERAGE, "rust"), { recursive: true });
const rust = run("cargo", [
  "llvm-cov",
  "--manifest-path",
  "src-tauri/Cargo.toml",
  "--html",
  "--output-dir",
  join(COVERAGE, "rust"),
]);
process.exit(rust);
