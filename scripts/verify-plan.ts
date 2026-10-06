import { existsSync, readdirSync } from "node:fs";
import { join } from "node:path";

export type VerificationStep = { name: string; command: string[] };
const STANDARD = ["format", "lint", "typecheck", "generated", "size"];
const ADVANCE = [...STANDARD, "build", "quality", "ipc", "test"];
const FULL = [...ADVANCE, "e2e"];
const STAGES = [...FULL, "desktop-build", "asr", "conversation-queue", "coverage"];
const DESKTOP = "src-tauri";
const TS_FORMAT = [
  "src/**/*.{ts,tsx}",
  "tests/**/*.{ts,tsx}",
  "scripts/**/*.{ts,tsx}",
  "vite.config.ts",
];

export const VERIFY_HELP = `Usage: bun scripts/verify.ts [advance|full|${STAGES.join("|")}] [options] [-- arguments]
No stage: run static checks, without builds or tests.
advance: add builds and unit/contract tests for build readiness; not required for checkpoint commit/push.
full: add offline provider/conversation E2E and desktop smoke (major updates).
Success prints exactly one OK; failure prints
the complete failing command output and stops without running later commands.

  --scope typescript|rust   Check only the selected language
  --package <directory>    Check one Rust package (implies --scope rust)
  --packages-only          Check Rust packages other than src-tauri
  --write                  Apply formatting (format stage only)
  --portable               Skip platform-specific frontend tests
  --                       Forward test arguments; requires an explicit language
                           and, for Rust, --package. Also accepts TypeScript
                           format paths with --scope typescript.

Examples:
  bun run --silent verify
  bun run --silent verify:advance
  bun run --silent verify:full
  bun run --silent verify lint
  bun run --silent verify format --write
  bun run --silent verify test --scope typescript -- tests/verify.test.ts
  bun run --silent verify test --package src-tauri -- --lib voice::
  bun run --silent verify --package crates/larm-session
  bun run --silent build
  bun run --silent build:rust
  bun run --silent build:desktop -- --debug
`;

/** Discover every local package instead of keeping a second, incomplete list. */
export function rustPackages(root: string): string[] {
  const packages = [DESKTOP];
  for (const parent of ["crates", "services"]) {
    const directory = join(root, parent);
    if (!existsSync(directory)) continue;
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      if (entry.isDirectory() && existsSync(join(directory, entry.name, "Cargo.toml"))) {
        packages.push(`${parent}/${entry.name}`);
      }
    }
  }
  return packages.sort();
}

export function verificationPlan(args: string[], root: string): VerificationStep[] {
  const options = [...args];
  const stage = options[0] && !options[0].startsWith("-") ? options.shift() : undefined;
  if (stage && ![...STAGES, "advance", "full"].includes(stage))
    throw new Error(`Unknown stage: ${stage}\n${VERIFY_HELP}`);
  let scope: string | undefined;
  let rustPackage: string | undefined;
  let packagesOnly = false;
  let write = false;
  let portable = false;
  let forwarded: string[] = [];
  while (options.length) {
    const option = options.shift();
    if (option === "--scope") {
      if (scope) throw new Error("--scope may only be specified once");
      scope = options.shift();
      if (scope !== "typescript" && scope !== "rust") throw new Error("Invalid --scope");
    } else if (option === "--package") {
      if (rustPackage) throw new Error("--package may only be specified once");
      rustPackage = options.shift()?.replace(/\\/g, "/").replace(/\/$/, "");
      if (!rustPackage) throw new Error("Missing --package directory");
    } else if (option === "--packages-only") packagesOnly = true;
    else if (option === "--write") write = true;
    else if (option === "--portable") portable = true;
    else if (option === "--") {
      forwarded = options.splice(0);
    } else throw new Error(`Unknown option: ${option}\n${VERIFY_HELP}`);
  }
  if (write && stage !== "format") throw new Error("--write requires the format stage");
  if ((rustPackage || packagesOnly) && scope === "typescript") {
    throw new Error("Rust package selection conflicts with --scope typescript");
  }
  if (rustPackage && packagesOnly) throw new Error("Choose --package or --packages-only");
  if (rustPackage || packagesOnly) scope = "rust";
  if (
    stage &&
    ["full", "e2e", "desktop-build", "asr", "conversation-queue", "coverage"].includes(stage) &&
    scope
  ) {
    throw new Error("This suite requires TypeScript and the desktop Rust package together");
  }
  if (portable && scope === "rust") throw new Error("--portable requires frontend tests");
  if (
    forwarded.length &&
    !(
      (stage === "test" && scope && (scope !== "rust" || rustPackage)) ||
      stage === "desktop-build" ||
      (stage === "format" && scope === "typescript")
    )
  ) {
    throw new Error(
      "Arguments require test --scope typescript, test --package, desktop-build, or format --scope typescript",
    );
  }
  const allPackages = rustPackages(root);
  if (rustPackage && !allPackages.includes(rustPackage)) {
    throw new Error(`Unknown Rust package: ${rustPackage}`);
  }
  const packages = rustPackage
    ? [rustPackage]
    : allPackages.filter((directory) => !packagesOnly || directory !== DESKTOP);
  const typescript = scope !== "rust";
  const rust = scope !== "typescript";
  const desktop = rust && packages.includes(DESKTOP);
  const steps: VerificationStep[] = [];
  const add = (name: string, command: string[]) => steps.push({ name, command });
  const bun = (name: string, ...args: string[]) => add(name, [process.execPath, ...args]);
  const bin = (name: string, executable: string, ...args: string[]) =>
    bun(name, "x", "--no-install", executable, ...args);
  const cargo = (name: string, command: string, directory: string, ...args: string[]) =>
    add(`${name} (${directory})`, [
      "cargo",
      command,
      "--manifest-path",
      `${directory}/Cargo.toml`,
      ...args,
    ]);
  const generated = () => {
    bin("generated contexts", "s11tnext", "build", "--check", "--release-profile", "development");
    bun("rendered contexts", "scripts/render-system-contexts.ts", "--check");
  };
  const typescriptCheck = () => {
    bin("typecheck (TypeScript)", "tsc", "--noEmit", "-p", "tsconfig.json");
    bin(
      "typecheck (Vite config)",
      "tsc",
      "--noEmit",
      "--incremental",
      "false",
      "--composite",
      "false",
      "-p",
      "tsconfig.node.json",
    );
  };
  const ipc = () =>
    cargo(
      "IPC contracts",
      "test",
      DESKTOP,
      "--locked",
      "--test",
      "ipc_contract_bindings",
      "--test",
      "voice_asr_contract_bindings",
    );
  const profile = !stage
    ? STANDARD
    : stage === "advance"
      ? ADVANCE
      : stage === "full"
        ? FULL
        : [stage];
  const individual = Boolean(stage && stage !== "advance" && stage !== "full");
  for (const selected of profile) {
    const before = steps.length;
    switch (selected) {
      case "format":
        if (typescript)
          bin(
            "format (TypeScript)",
            "oxfmt",
            write ? "--write" : "--check",
            ...(forwarded.length ? forwarded : TS_FORMAT),
          );
        if (rust) {
          for (const directory of packages)
            cargo("format", "fmt", directory, ...(write ? [] : ["--check"]));
        }
        break;
      case "lint":
        if (typescript)
          bin(
            "lint (TypeScript)",
            "oxlint",
            "src",
            "tests",
            "scripts",
            "vite.config.ts",
            "--deny-warnings",
          );
        if (rust) {
          for (const directory of packages) {
            if (directory === DESKTOP) bun("lint (src-tauri)", "scripts/clippy-ratchet.ts");
            else
              cargo(
                "lint",
                "clippy",
                directory,
                "--locked",
                "--all-targets",
                "--",
                "-D",
                "warnings",
              );
          }
        }
        break;
      case "typecheck":
        if (typescript) typescriptCheck();
        if (rust) {
          for (const directory of packages)
            cargo("typecheck", "check", directory, "--locked", "--all-targets");
        }
        break;
      case "generated":
        if (typescript) generated();
        break;
      case "size":
        if (typescript) bun("module size", "scripts/module-size.ts", "check");
        break;
      case "build":
        if (typescript) {
          if (individual) {
            typescriptCheck();
            generated();
          }
          bin("frontend build", "vite", "build");
        }
        if (rust) {
          for (const directory of packages)
            cargo("build", "build", directory, "--locked", "--all-targets");
        }
        break;
      case "quality":
        if (!scope) {
          bun("conversation quality", "scripts/conversation-quality-eval.ts", "check");
          cargo(
            "quality runtime contract",
            "test",
            DESKTOP,
            "--locked",
            "--features",
            "quality-eval-harness",
            "quality_eval::tests",
          );
        }
        break;
      case "desktop-build":
        bin("desktop build", "tauri", "build", ...forwarded);
        break;
      case "ipc":
        if (desktop) ipc();
        break;
      case "test":
        if (typescript) {
          if (individual && !forwarded.length) generated();
          if (forwarded.length) bun("frontend tests (filtered)", "test", ...forwarded);
          else
            bun(
              "frontend tests",
              "scripts/frontend-tests.ts",
              "--fail-fast",
              ...(portable ? ["--portable"] : []),
            );
        }
        if (rust) {
          for (const directory of packages)
            cargo("tests", "test", directory, "--locked", ...forwarded);
        }
        break;
      case "e2e":
        cargo(
          "provider and conversation E2E",
          "test",
          DESKTOP,
          "--locked",
          "--features",
          "provider-unit-test-harness,conversation-queue-e2e",
          "--test",
          "provider_unit_test_e2e",
          "--test",
          "conversation_queue_e2e",
          "--test",
          "conversation_queue_followup_failure",
          "--test",
          "qwen_realtime_asr_e2e",
        );
        bun("desktop E2E smoke", "scripts/desktop-smoke.ts");
        break;
      case "asr":
        bun(
          "ASR contracts",
          "test",
          "tests/conversation-asr-continuous.test.ts",
          "tests/settings-review.test.ts",
          "tests/voice-asr-packetizer.test.ts",
          "tests/qwen-realtime-asr-ipc.test.ts",
        );
        bun(
          "ASR frontend",
          "test",
          "tests/browser-voice-capture.test.ts",
          "tests/provider-unit-asr.test.tsx",
          "tests/audio-backend.test.ts",
        );
        cargo(
          "ASR E2E",
          "test",
          DESKTOP,
          "--locked",
          "--features",
          "provider-unit-test-harness",
          "--test",
          "qwen_realtime_asr_e2e",
        );
        break;
      case "conversation-queue":
        cargo(
          "conversation queue E2E",
          "test",
          DESKTOP,
          "--locked",
          "--features",
          "conversation-queue-e2e",
          "--test",
          "conversation_queue_e2e",
          "--test",
          "conversation_queue_followup_failure",
        );
        cargo(
          "queue and speech contracts",
          "test",
          DESKTOP,
          "--locked",
          "--test",
          "task_queue_contract",
          "--test",
          "conversation_queue_progress_contract",
          "--test",
          "tts_recovery_contract",
          "--test",
          "tts_streaming_contract",
        );
        bun(
          "continuous ASR contracts",
          "test",
          "tests/conversation-asr-continuous.test.ts",
          "tests/qwen-realtime-asr-ipc.test.ts",
        );
        bun("conversation queue frontend", "test", "tests/conversation-queue-page.test.tsx");
        break;
      case "coverage":
        bun("coverage reports", "scripts/test-coverage.ts");
        break;
    }
    if (individual && steps.length === before)
      throw new Error(`${stage} does not apply to the selected scope`);
  }
  return steps;
}
