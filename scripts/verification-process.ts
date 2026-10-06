import { spawn, spawnSync } from "node:child_process";
import { signalSmokeProcess } from "./desktop-smoke-signals";
import { verificationGroupLive, type VerificationLock } from "./verification-lock";

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
    force = setTimeout(() => kill("SIGKILL"), 2_000);
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
