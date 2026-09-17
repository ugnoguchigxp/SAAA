import { mkdirSync, writeFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";

export type AutomatedCheck = {
  id: "local" | "rust-packages" | "spec";
  elapsedMs: number;
  result: "passed" | "failed";
};

export class ProductReadinessError extends Error {
  constructor(public readonly code: "usage-error" | "report-exists" | "report-write-failed") {
    super(code);
  }
}

export function parseProductReadinessArguments(args: string[]): {
  reportDirectory: string;
} {
  if (args.length !== 2 || args[0] !== "--report-dir" || !isAbsolute(args[1] ?? "")) {
    throw new ProductReadinessError("usage-error");
  }
  return { reportDirectory: args[1]! };
}

export function buildProductReadinessReport(input: {
  checks: AutomatedCheck[];
  commit: string;
  workingTree: "clean" | "dirty";
  os: string;
  arch: string;
  generatedAt?: string;
}) {
  const automatedResult = input.checks.every(({ result }) => result === "passed")
    ? "passed"
    : "failed";
  const hasVerifiedCommit = /^[0-9a-f]{40}$/.test(input.commit);
  return {
    format: "saaa-product-readiness-automated-v1",
    generatedAt: input.generatedAt ?? new Date().toISOString(),
    commit: input.commit,
    workingTree: input.workingTree,
    environment: { os: input.os, arch: input.arch },
    automatedResult,
    releaseResult:
      automatedResult === "passed" && input.workingTree === "clean" && hasVerifiedCommit
        ? "manual-acceptance-pending"
        : "blocked",
    checks: input.checks,
    scenarioCoverage: {
      U01: ["tests/setup-checklist.test.tsx", "tests/provider-card-async.test.tsx"],
      U02: ["tests/interrupted-streaming-ui.test.ts", "tests/ipc-event-order.test.ts"],
      U03: ["tests/provider-card-async.test.tsx", "tests/conversation-timeout.test.ts"],
      U04: ["persistence::runs::tests::startup_reconciles_running_work"],
      U05: ["tests/microphone.test.ts", "tests/ambient-voice-session.test.tsx"],
      U06: ["tests/meeting-contracts.test.ts", "meeting::tests::explicit_save_is_transactional"],
      U07: ["tests/generative-ui-boundary.test.tsx", "tests/generative-ui-contracts.test.ts"],
      U08: ["tests/keyboard-interactions.test.tsx"],
      U09: ["tests/personal-state-eval.test.ts", "memory::personal_state::tests"],
    },
    manualResult: "pending",
    contentIncluded: false,
  } as const;
}

export function writeProductReadinessReport(
  directory: string,
  report: ReturnType<typeof buildProductReadinessReport>,
): void {
  mkdirSync(directory, { recursive: true, mode: 0o700 });
  try {
    writeFileSync(join(directory, "automated.json"), `${JSON.stringify(report, null, 2)}\n`, {
      mode: 0o600,
      flag: "wx",
    });
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    throw new ProductReadinessError(code === "EEXIST" ? "report-exists" : "report-write-failed");
  }
}
