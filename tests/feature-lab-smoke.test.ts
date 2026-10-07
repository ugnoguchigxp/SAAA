import { expect, test } from "bun:test";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  smokeDiagnostics,
  smokeExitCode,
  type OwnedProcess,
  type Probe,
  type SignalResult,
} from "../scripts/feature-lab-smoke-reap";
import {
  cleanupOwnedResources,
  trackOwnedProcesses,
  type ProcessRow,
} from "../scripts/verification-process";

async function finishOwnedResources(input: {
  processes: OwnedProcess[];
  directories: string[];
  closeVite: () => Promise<void>;
  viteListening: () => Promise<boolean>;
  sleep?: (ms: number) => Promise<void>;
}) {
  let clock = 0;
  return cleanupOwnedResources({
    ...input,
    collect: () => input.processes,
    sleep: async (ms) => {
      clock += ms;
      await input.sleep?.(ms);
    },
    now: () => clock,
  });
}

const reapOwned = (processes: OwnedProcess[], sleep: (ms: number) => Promise<void>) =>
  finishOwnedResources({
    processes,
    directories: [],
    closeVite: async () => {},
    viteListening: async () => false,
    sleep,
  });

function fake(
  pid: number,
  kind: string,
  behavior: "exit" | "remain" | "unknown" | "deny-then-die" | "absent",
): OwnedProcess {
  let state: Probe = "alive";
  return {
    pid,
    kind,
    signal(signal): SignalResult {
      if (behavior === "unknown") return "denied";
      if (behavior === "deny-then-die") {
        state = "dead";
        return "denied";
      }
      if (behavior === "absent") {
        state = "dead";
        return "absent";
      }
      if (behavior === "exit" && (signal === "SIGTERM" || signal === "SIGKILL")) state = "dead";
      return state === "dead" ? "absent" : "signaled";
    },
    probe: () => (behavior === "unknown" ? "unknown" : state),
  };
}

const sleep = async () => {};
const emptyFinish = {
  ownedProcessesAlive: [],
  viteListening: false,
  directoriesRemaining: [],
  errors: [],
};

test("smoke succeeds only after owned processes, vite, and temp dirs are gone", async () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-smoke-clean-"));
  const finish = await finishOwnedResources({
    processes: [fake(11, "host", "exit"), fake(12, "chrome", "exit")],
    directories: [directory],
    closeVite: async () => {},
    viteListening: async () => false,
    sleep,
  });
  expect(finish.ownedProcessesAlive).toEqual([]);
  expect(existsSync(directory)).toBe(false);
  expect(smokeExitCode({ signal: 0, pageOk: true, finish })).toBe(0);
  expect(smokeDiagnostics(finish)).toBe("");
});

test("page failure, a missing browser, or a busy port is not OK", () => {
  for (const pageOk of [false]) {
    expect(smokeExitCode({ signal: 0, pageOk, finish: emptyFinish })).toBe(1);
  }
  expect(
    smokeExitCode({ signal: 0, pageOk: true, finish: { ...emptyFinish, viteListening: true } }),
  ).toBe(1);
});

test("a process that survives, or cannot be confirmed dead, fails and keeps its directory", async () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-smoke-live-"));
  try {
    const finish = await finishOwnedResources({
      processes: [fake(21, "chrome", "remain"), fake(22, "descendant", "unknown")],
      directories: [directory],
      closeVite: async () => {},
      viteListening: async () => false,
      sleep,
    });
    expect(finish.ownedProcessesAlive.map((item) => item.pid).sort()).toEqual([21, 22]);
    expect(existsSync(directory)).toBe(true);
    expect(smokeExitCode({ signal: 0, pageOk: true, finish })).toBe(1);
    expect(smokeDiagnostics(finish)).toContain("pid=21 kind=chrome");
    expect(smokeDiagnostics(finish)).toContain("pid=22 kind=descendant");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("EPERM is not proof of exit, and ESRCH is absence", async () => {
  const denied = await reapOwned([fake(31, "host", "unknown")], sleep);
  expect(denied.ownedProcessesAlive).toEqual([{ pid: 31, kind: "host" }]);
  expect(denied.errors.join("\n")).toContain("signal denied");
  const gone = await reapOwned([fake(32, "chrome", "deny-then-die")], sleep);
  expect(gone.ownedProcessesAlive).toEqual([]);
  expect(gone.errors).toEqual([]);
  const absent = await reapOwned([fake(33, "descendant", "absent")], sleep);
  expect(absent.ownedProcessesAlive).toEqual([]);
  expect(absent.errors).toEqual([]);
});

test("SIGINT and SIGTERM still exit 130 and 143 after cleanup", () => {
  expect(smokeExitCode({ signal: 130, pageOk: true, finish: emptyFinish })).toBe(130);
  expect(smokeExitCode({ signal: 143, pageOk: false, finish: emptyFinish })).toBe(143);
});

test("cleanup discovers and terminates a new child during the grace period", async () => {
  const child = fake(42, "descendant", "exit");
  let parentAlive = true;
  let clock = 0;
  const parent: OwnedProcess = {
    pid: 41,
    kind: "host",
    probe: () => (parentAlive ? "alive" : "dead"),
    signal: () => {
      if (clock > 0) parentAlive = false;
      return "signaled";
    },
  };
  const finish = await cleanupOwnedResources({
    collect: () => (clock > 0 ? [parent, child] : [parent]),
    directories: [],
    closeVite: async () => {},
    viteListening: async () => false,
    sleep: async (ms) => {
      clock += ms;
      parentAlive = false;
    },
    now: () => clock,
  });
  expect(finish.ownedProcessesAlive).toEqual([]);
  expect(child.probe()).toBe("dead");
  expect(smokeExitCode({ signal: 0, pageOk: true, finish })).toBe(0);
});

test("ownership retains reparented separate groups and excludes an existing browser", () => {
  let rows: ProcessRow[] = [{ pid: 77, parent: 10, group: 77, state: "S" }];
  const tracker = trackOwnedProcesses(10, () => rows);
  rows = [
    ...rows,
    { pid: 11, parent: 10, group: 11, state: "S" },
    { pid: 12, parent: 11, group: 12, state: "S" },
  ];
  expect(tracker.collect().map((item) => item.pid)).toEqual([11, 12]);
  rows = [
    rows[0]!,
    { pid: 12, parent: 1, group: 12, state: "S" },
    { pid: 13, parent: 1, group: 12, state: "S" },
  ];
  expect(
    tracker
      .collect()
      .filter((item) => item.probe() === "alive")
      .map((item) => item.pid),
  ).toEqual([12, 13]);
});

test("a stuck Vite close and uncertain ownership preserve temporary data and fail", async () => {
  const directory = mkdtempSync(join(tmpdir(), "saaa-smoke-uncertain-"));
  let clock = 0;
  try {
    const finish = await cleanupOwnedResources({
      collect: () => [],
      directories: [directory],
      trackingErrors: new Set(["ownership unavailable"]),
      closeVite: () => new Promise(() => {}),
      viteListening: async () => true,
      sleep: async (ms) => {
        clock += ms;
      },
      now: () => clock,
    });
    expect(clock).toBe(12_000);
    expect(existsSync(directory)).toBe(true);
    expect(smokeExitCode({ signal: 0, pageOk: true, finish })).toBe(1);
    expect(finish.errors).toContain("ownership unavailable");
    expect(finish.errors.join("\n")).toContain("Vite");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
