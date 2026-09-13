import { afterEach, describe, expect, test } from "bun:test";
import { chmodSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  countSessions,
  knownObservationIdentifiers,
  median,
  readStreamBytes,
  runtimeCategory,
  sampleRssKiB,
  validateSoakObservation,
  waitForCheckpoint,
} from "../scripts/larm-readiness/live";
import {
  encodeU32,
  encodeU64,
  resultStrength,
  testNameForMode,
} from "../scripts/larm-readiness/bundle";
import { failureCode, removeRustFragment, run } from "../scripts/larm-readiness/runner";
import {
  REPORT_FILENAMES,
  RunnerError,
  atomicWriteReport,
  emptyReport,
} from "../scripts/larm-readiness.ts";
import type {
  DatabaseObservation,
  ProviderSessionRow,
  RuntimeRow,
} from "../scripts/larm-readiness/database";

const temporaryDirectories: string[] = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0))
    rmSync(directory, { recursive: true, force: true });
});

function identity() {
  return {
    saaaCommit: "1234567",
    artifactSha256: "a".repeat(64),
    manifestSha256: "b".repeat(64),
    larmContractCommit: "7dca7c3",
    deploymentRevision: "revision-1",
  };
}

function session(overrides: Partial<ProviderSessionRow>): ProviderSessionRow {
  return {
    id: "session-1",
    runtime_run_id: "run-1",
    provider_id: "larm-local",
    provider_kind: "larm",
    allocation_id: "alloc-1",
    selected_runtime_id: "qwen-general",
    request_id: "req-1",
    fallback_used: 0,
    route_id: "llm-default",
    selection_reason: "primary",
    output_started: 1,
    failure_kind: null,
    release_status: "released",
    status: "completed",
    ...overrides,
  };
}

function runRow(overrides: Partial<RuntimeRow>): RuntimeRow {
  return {
    id: "run-1",
    conversation_id: "conversation-1",
    provider_id: "larm-local",
    status: "completed",
    ...overrides,
  };
}

describe("LARM live helpers", () => {
  test("classifies runtimes, medians, and soak observations", () => {
    expect(runtimeCategory("qwen-general")).toBe("resident-default");
    expect(runtimeCategory("other")).toBe("unknown");
    expect(median([1, 3, 2])).toBe(2);
    expect(median([1, 2, 3, 4])).toBe(2.5);
    expect(() => median([])).toThrow(RunnerError);
    const observation: DatabaseObservation = {
      runs: [runRow({ id: "run-1" }), runRow({ id: "run-2", status: "cancelled" })],
      sessions: [
        session({ id: "s1", runtime_run_id: "run-1", allocation_id: "a1" }),
        session({
          id: "s2",
          runtime_run_id: "run-2",
          allocation_id: "a2",
          status: "cancelled",
          failure_kind: "cancelled",
        }),
      ],
    };
    expect(validateSoakObservation(observation, "larm-local")).toEqual({
      completed: 1,
      cancelled: 1,
    });
    expect(countSessions(observation, (item) => item.status === "cancelled")).toBe(1);
    expect(knownObservationIdentifiers(observation)).toContain("run-1");
  });

  test("reads stream bytes, samples the current process RSS, and times out a checkpoint", async () => {
    const bytes = await readStreamBytes(
      new ReadableStream({
        start(controller) {
          controller.enqueue(Buffer.from("abc"));
          controller.close();
        },
      }),
      16,
    );
    expect(bytes.toString()).toBe("abc");
    await expect(
      readStreamBytes(
        new ReadableStream({
          start(controller) {
            controller.enqueue(Buffer.from("too-long"));
            controller.close();
          },
        }),
        3,
      ),
    ).rejects.toThrow(RunnerError);
    expect(await sampleRssKiB(process.pid)).toBeGreaterThan(0);
    const application = {
      child: { exitCode: null, pid: process.pid },
      stdoutScanner: { detected: false },
      stderrScanner: { detected: false },
    };
    await expect(
      waitForCheckpoint(application as never, "waiting", () => false, performance.now() - 1),
    ).rejects.toThrow(RunnerError);
    await waitForCheckpoint(application as never, "ready", () => true, performance.now() + 1_000);
  });
});

describe("LARM bundle and runner helpers", () => {
  test("encodes integers and names live suites", () => {
    expect(encodeU32(1).readUInt32BE()).toBe(1);
    expect(Number(encodeU64(2).readBigUInt64BE())).toBe(2);
    expect(resultStrength("failed")).toBe(2);
    expect(testNameForMode("preflight")).toContain("live_preflight");
    expect(failureCode(new RunnerError(2, "redaction-failed", "failed"))).toBe("redaction-failed");
    expect(failureCode(new RunnerError(2, "not-a-code", "failed"))).toBe("internal");
  });

  test("aggregates four reports and rejects a malformed rust fragment", async () => {
    const directory = realpathSync(mkdtempSync(join(tmpdir(), "saaa-larm-runner-")));
    chmodSync(directory, 0o700);
    temporaryDirectories.push(directory);
    const id = identity();
    for (const mode of ["preflight", "functional", "soak-30m", "soak-2h"] as const) {
      atomicWriteReport(
        join(directory, REPORT_FILENAMES[mode]),
        emptyReport(id, mode, "failed", ["internal"]),
      );
    }
    const previousToken = process.env.LARM_API_TOKEN;
    const previousUrl = process.env.SAAA_LARM_CANARY_BASE_URL;
    const previousKey = process.env.SAAA_PROVIDER_LOCAL_OPENAI_COMPATIBLE_API_KEY;
    delete process.env.LARM_API_TOKEN;
    delete process.env.SAAA_LARM_CANARY_BASE_URL;
    delete process.env.SAAA_PROVIDER_LOCAL_OPENAI_COMPATIBLE_API_KEY;
    try {
      const outcome = await run({ command: "report", reportDirectory: directory });
      expect(outcome.mode).toBe("aggregate");
      expect(outcome.result).toBe("failed");
    } finally {
      if (previousToken) process.env.LARM_API_TOKEN = previousToken;
      if (previousUrl) process.env.SAAA_LARM_CANARY_BASE_URL = previousUrl;
      if (previousKey) process.env.SAAA_PROVIDER_LOCAL_OPENAI_COMPATIBLE_API_KEY = previousKey;
    }
    const fragment = join(directory, "fragment.json");
    writeFileSync(fragment, "{}", { mode: 0o600 });
    expect(() => removeRustFragment(fragment)).not.toThrow();
  });
});
