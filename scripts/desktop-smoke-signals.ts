import { spawnSync, type ChildProcess } from "node:child_process";

export function signalSmokeProcess(child: ChildProcess, signal: NodeJS.Signals) {
  if (!child.pid) return;
  try {
    if (process.platform === "win32") child.kill(signal);
    else process.kill(-child.pid, signal);
  } catch (cause) {
    const code = (cause as NodeJS.ErrnoException).code;
    if (code === "ESRCH") return;
    // Darwin can return EPERM while a killed group contains only unreaped zombies.
    // Never suppress a permission failure for a group with a living process.
    if (code === "EPERM" && process.platform === "darwin") {
      const result = spawnSync("/bin/ps", ["-axo", "pgid=,stat="], { encoding: "utf8" });
      if (result.status === 0) {
        const members = result.stdout
          .trim()
          .split("\n")
          .map((line) => line.trim().split(/\s+/))
          .filter(([group]) => Number(group) === child.pid);
        if (members.every(([, state]) => state.startsWith("Z"))) return;
      }
    }
    throw cause;
  }
}
