import { spawn } from "node:child_process";
import { signalSmokeProcess } from "./desktop-smoke-signals";

const LOG_LIMIT = 256_000;

export function startSmokeProcess(
  command: string[],
  root: string,
  env = process.env,
  detached = true,
) {
  const child = spawn(command[0], command.slice(1), {
    cwd: root,
    env,
    detached: detached && process.platform !== "win32",
    stdio: ["ignore", "pipe", "pipe"],
  });
  const events: { event: string; elapsedMs: number }[] = [];
  const started = Date.now();
  const record = (event: string) => events.push({ event, elapsedMs: Date.now() - started });
  child.once("spawn", () => record("spawn"));
  child.once("error", () => record("error"));
  child.once("exit", () => record("exit"));
  child.once("close", () => record("close"));
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => {
    stdout = (stdout + chunk).slice(-LOG_LIMIT);
  });
  child.stderr.on("data", (chunk: string) => {
    stderr = (stderr + chunk).slice(-LOG_LIMIT);
  });
  const exited = new Promise<number>((resolve, reject) => {
    child.once("error", reject);
    child.once("exit", (code) => resolve(code ?? 1));
  });
  const closed = new Promise<void>((resolve) => child.once("close", () => resolve()));
  const kill = (signal: NodeJS.Signals) => {
    if (detached) signalSmokeProcess(child, signal);
    else child.kill(signal);
  };
  return { child, exited, closed, kill, events, logs: () => ({ stdout, stderr }) };
}

export type SmokeProcess = ReturnType<typeof startSmokeProcess>;
export async function terminateSmokeProcess(child: SmokeProcess): Promise<void> {
  child.kill("SIGTERM");
  const force = setTimeout(() => child.kill("SIGKILL"), 2_000);
  try {
    await child.closed;
  } finally {
    clearTimeout(force);
    // Parent close only accounts for inherited pipes. A descendant with its own
    // output may still be alive after ignoring SIGTERM.
    child.kill("SIGKILL");
  }
}
