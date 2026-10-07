import { spawn, spawnSync } from "node:child_process";
import { signalSmokeProcess } from "./desktop-smoke-signals";
import { verificationGroupLive, type VerificationLock } from "./verification-lock";
import { existsSync, rmSync } from "node:fs";
import type { OwnedProcess, SmokeFinish } from "./feature-lab-smoke-reap";

export type ProcessRow = { pid: number; parent: number; group: number; state: string };

function processRows(): ProcessRow[] {
  const result = spawnSync("ps", ["-axo", "pid=,ppid=,pgid=,stat="], {
    encoding: "utf8",
    timeout: 1_000,
  });
  if (result.status !== 0) throw new Error("could not list owned descendants");
  return result.stdout
    .trim()
    .split("\n")
    .map((line) => {
      const [pid, parent, group, state] = line.trim().split(/\s+/);
      if (![pid, parent, group].every((value) => /^\d+$/.test(value ?? "")) || !state)
        throw new Error("could not parse owned descendants");
      return { pid: Number(pid), parent: Number(parent), group: Number(group), state };
    });
}

/** Start before launching resources. Keep discovered descendants even if they reparent. */
export function trackOwnedProcesses(owner = process.pid, list = processRows) {
  const baseline = new Set(list().map((row) => row.pid));
  const owned = new Map<number, OwnedProcess>();
  const groups = new Set<number>();
  let rows: ProcessRow[] | undefined;
  const errors = new Set<string>();
  const register = (pid: number, kind: string, detached = false) => {
    if (!pid || pid === owner || baseline.has(pid)) return;
    if (detached) groups.add(pid);
    const previous = owned.get(pid);
    if (previous) {
      previous.kind = kind;
      return;
    }
    owned.set(pid, {
      pid,
      kind,
      signal(signal) {
        try {
          process.kill(pid, signal);
          return "signaled";
        } catch (cause) {
          return (cause as NodeJS.ErrnoException).code === "ESRCH" ? "absent" : "denied";
        }
      },
      probe: () =>
        !rows
          ? "unknown"
          : rows.some((row) => row.pid === pid && !row.state.startsWith("Z"))
            ? "alive"
            : "dead",
    });
  };
  const collect = () => {
    try {
      rows = list();
      let changed = true;
      while (changed) {
        changed = false;
        for (const row of rows) {
          if (row.pid === owner || baseline.has(row.pid)) continue;
          if (row.parent === owner || owned.has(row.parent) || groups.has(row.group)) {
            if (!owned.has(row.pid)) {
              register(row.pid, "descendant");
              changed = true;
            }
            if (row.group === row.pid) groups.add(row.group);
          }
        }
      }
    } catch {
      rows = undefined;
      errors.add("could not confirm owned descendants exited");
    }
    return [...owned.values()];
  };
  return { register, collect, errors };
}

export async function cleanupOwnedResources(input: {
  collect: () => OwnedProcess[];
  closeVite: () => Promise<void>;
  viteListening: () => Promise<boolean>;
  directories: string[];
  sleep?: (ms: number) => Promise<void>;
  now?: () => number;
  trackingErrors?: Set<string>;
}): Promise<SmokeFinish> {
  const sleep = input.sleep ?? Bun.sleep;
  const now = input.now ?? (() => performance.now());
  const errors: string[] = [];
  // Close Vite alongside process termination; a stuck close must not hang the smoke.
  let viteClosed = false;
  void Promise.resolve()
    .then(input.closeVite)
    .then(
      () => {
        viteClosed = true;
      },
      () => {
        errors.push("Vite close failed");
      },
    );
  let owned: OwnedProcess[] = [];
  for (const [signal, budget] of [
    ["SIGTERM", 10_000],
    ["SIGKILL", 2_000],
  ] as const) {
    const signaled = new Set<number>();
    const deadline = now() + budget;
    while (true) {
      owned = input.collect();
      for (const item of owned) {
        if (item.probe() === "dead" || signaled.has(item.pid)) continue;
        signaled.add(item.pid);
        if (item.signal(signal) === "denied")
          errors.push(`signal denied pid=${item.pid} kind=${item.kind} signal=${signal}`);
      }
      if (owned.every((item) => item.probe() === "dead") && viteClosed) break;
      if (now() >= deadline) break;
      await sleep(Math.min(50, deadline - now()));
    }
    if (owned.every((item) => item.probe() === "dead") && viteClosed) break;
  }
  owned = input.collect();
  const ownedProcessesAlive = owned
    .filter((item) => item.probe() !== "dead")
    .map(({ pid, kind }) => ({ pid, kind }));
  // A later independent probe can confirm exit after EPERM.
  const unresolvedErrors = errors.filter(
    (error) =>
      !error.startsWith("signal denied") ||
      ownedProcessesAlive.some((item) => error.includes(`pid=${item.pid} `)),
  );
  for (const item of owned)
    if (item.probe() === "unknown")
      unresolvedErrors.push(`could not confirm pid=${item.pid} kind=${item.kind} exited`);
  unresolvedErrors.push(...(input.trackingErrors ?? []));
  if (!viteClosed)
    unresolvedErrors.push("Vite close was not confirmed within the cleanup deadline");
  let viteListening = true;
  try {
    viteListening = await input.viteListening();
  } catch {
    unresolvedErrors.push("Vite listener exit could not be confirmed");
  }
  if (viteListening) unresolvedErrors.push("Vite listener is still open");
  if (
    ownedProcessesAlive.length === 0 &&
    viteClosed &&
    !viteListening &&
    unresolvedErrors.length === 0
  ) {
    for (const directory of input.directories) {
      try {
        rmSync(directory, { recursive: true, force: true });
      } catch {
        unresolvedErrors.push(`could not remove temporary directory: ${directory}`);
      }
    }
  }
  const directoriesRemaining = input.directories.filter((directory) => existsSync(directory));
  return { ownedProcessesAlive, viteListening, directoriesRemaining, errors: unresolvedErrors };
}

export async function runVerificationProcess(
  command: string[],
  cwd: string,
  lock: VerificationLock,
  signal: AbortSignal,
  output: number | "inherit",
): Promise<number> {
  signal.throwIfAborted();
  const child = spawn(command[0], command.slice(1), {
    cwd,
    env: lock.env,
    // Nested verification stays in the outer group so cancellation also kills descendants.
    detached: !lock.inherited && process.platform !== "win32",
    stdio: ["ignore", output, output],
  });
  const kill = (value: NodeJS.Signals) => {
    if (!child.pid) return;
    if (process.platform === "win32" && !lock.inherited) {
      spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
    } else if (lock.inherited) child.kill(value);
    else signalSmokeProcess(child, value);
  };
  let force: ReturnType<typeof setTimeout> | undefined;
  const abort = () => {
    kill("SIGTERM");
    // lab-smoke owns detached host/browser groups and needs its 10s + 2s cleanup
    // window before the caller can forcibly terminate the owner itself.
    const smoke = command.some((arg) => /(?:^|[/\\])feature-lab-smoke\.ts$/.test(arg));
    force = setTimeout(() => kill("SIGKILL"), smoke ? 15_000 : 2_000);
  };
  signal.addEventListener("abort", abort, { once: true });
  try {
    lock.track(child.pid);
    return await new Promise<number>((done, fail) => {
      child.once("error", fail);
      child.once("close", (code) => done(code ?? 1));
    });
  } finally {
    signal.removeEventListener("abort", abort);
    if (force) clearTimeout(force);
    // Children can outlive a successfully exited parent, too. Clean the whole group
    // before releasing the build lock, including children with redirected output.
    if (!lock.inherited) {
      kill("SIGKILL");
      while (verificationGroupLive(child.pid)) await Bun.sleep(25);
    }
    lock.track();
  }
}
