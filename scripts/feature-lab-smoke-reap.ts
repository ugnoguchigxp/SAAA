import { existsSync, rmSync } from "node:fs";

export type Probe = "alive" | "dead" | "unknown";
export type SignalResult = "signaled" | "absent" | "denied";
export type OwnedProcess = {
  pid: number;
  kind: string;
  signal: (signal: NodeJS.Signals) => SignalResult;
  probe: () => Probe;
};

export type SmokeFinish = {
  ownedProcessesAlive: Array<{ pid: number; kind: string }>;
  viteListening: boolean;
  directoriesRemaining: string[];
  errors: string[];
};

export function ownedPid(pid: number, kind: string): OwnedProcess {
  return {
    pid,
    kind,
    signal(signal) {
      if (!pid) return "absent";
      const send = (target: number) => {
        try {
          process.kill(target, signal);
          return "signaled" as const;
        } catch (cause) {
          const code = (cause as NodeJS.ErrnoException).code;
          if (code === "ESRCH") return "absent" as const;
          return "denied" as const;
        }
      };
      const group = send(-pid);
      return group === "denied" ? send(pid) : group;
    },
    probe() {
      if (!pid) return "dead";
      try {
        process.kill(pid, 0);
        return "alive";
      } catch (cause) {
        const code = (cause as NodeJS.ErrnoException).code;
        if (code === "ESRCH") return "dead";
        return code === "EPERM" ? "alive" : "unknown";
      }
    },
  };
}

export function smokeDiagnostics(finish: SmokeFinish): string {
  const alive = finish.ownedProcessesAlive.map(
    (item) => `owned process still alive: pid=${item.pid} kind=${item.kind}`,
  );
  return [...alive, ...finish.errors.map((error) => `cleanup: ${error}`)].join("\n");
}

export function smokeExitCode(input: {
  signal: 0 | 130 | 143;
  pageOk: boolean;
  finish: SmokeFinish;
}): number {
  if (input.signal) return input.signal;
  const failed =
    !input.pageOk ||
    input.finish.ownedProcessesAlive.length > 0 ||
    input.finish.viteListening ||
    input.finish.directoriesRemaining.length > 0 ||
    input.finish.errors.length > 0;
  return failed ? 1 : 0;
}

export async function reapOwned(
  processes: OwnedProcess[],
  sleep: (ms: number) => Promise<void> = (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
): Promise<Pick<SmokeFinish, "ownedProcessesAlive" | "errors">> {
  const errors: string[] = [];
  await signalAndWait(processes, "SIGTERM", 10_000, sleep, errors);
  const remaining = processes.filter((item) => item.probe() !== "dead");
  if (remaining.length) await signalAndWait(remaining, "SIGKILL", 2_000, sleep, errors);
  const ownedProcessesAlive = processes.flatMap((item) => {
    const state = item.probe();
    if (state === "dead") return [];
    if (state === "unknown")
      errors.push(`could not confirm pid=${item.pid} kind=${item.kind} exited`);
    return [{ pid: item.pid, kind: item.kind }];
  });
  return { ownedProcessesAlive, errors };
}

async function signalAndWait(
  processes: OwnedProcess[],
  signal: NodeJS.Signals,
  budgetMs: number,
  sleep: (ms: number) => Promise<void>,
  errors: string[],
): Promise<void> {
  const denied: OwnedProcess[] = [];
  for (const item of processes) {
    if (item.probe() === "dead") continue;
    if (item.signal(signal) === "denied") denied.push(item);
  }
  let waited = 0;
  while (waited < budgetMs && processes.some((item) => item.probe() === "alive")) {
    await sleep(50);
    waited += 50;
  }
  for (const item of denied) {
    if (item.probe() !== "dead")
      errors.push(`EPERM pid=${item.pid} kind=${item.kind} signal=${signal}`);
  }
}

export function expandOwned(processes: OwnedProcess[]): {
  processes: OwnedProcess[];
  errors: string[];
} {
  const listed = Bun.spawnSync(["ps", "-axo", "pid=,ppid=,pgid="], {
    stdout: "pipe",
    stderr: "pipe",
  });
  if (listed.exitCode !== 0) return { processes, errors: ["could not list owned descendants"] };
  const rows = new TextDecoder()
    .decode(listed.stdout)
    .trim()
    .split("\n")
    .map((line) => line.trim().split(/\s+/).map(Number))
    .filter((row) => row.length >= 3 && row.every((value) => Number.isInteger(value)));
  const leaders = new Set(processes.map((item) => item.pid));
  const wanted = new Set(leaders);
  let grew = true;
  while (grew) {
    grew = false;
    for (const [pid, parent, group] of rows) {
      if (!pid || pid === process.pid || wanted.has(pid)) continue;
      if (wanted.has(parent) || leaders.has(group)) {
        wanted.add(pid);
        grew = true;
      }
    }
  }
  const extras = [...wanted]
    .filter((pid) => !leaders.has(pid))
    .map((pid) => ownedPid(pid, "descendant"));
  return { processes: [...processes, ...extras], errors: [] };
}

export async function finishOwnedResources(input: {
  processes: OwnedProcess[];
  closeVite: () => Promise<void>;
  viteListening: () => Promise<boolean>;
  directories: string[];
  sleep?: (ms: number) => Promise<void>;
}): Promise<SmokeFinish> {
  const reaped = await reapOwned(input.processes, input.sleep);
  const errors = [...reaped.errors];
  try {
    await input.closeVite();
  } catch (cause) {
    errors.push(cause instanceof Error ? cause.message : String(cause));
  }
  const viteListening = await input.viteListening();
  const directoriesRemaining: string[] = [];
  if (reaped.ownedProcessesAlive.length === 0) {
    for (const directory of input.directories) {
      try {
        rmSync(directory, { recursive: true, force: true });
      } catch (cause) {
        errors.push(cause instanceof Error ? cause.message : String(cause));
        directoriesRemaining.push(directory);
      }
    }
  } else
    directoriesRemaining.push(...input.directories.filter((directory) => existsSync(directory)));
  return {
    ownedProcessesAlive: reaped.ownedProcessesAlive,
    viteListening,
    directoriesRemaining,
    errors,
  };
}
