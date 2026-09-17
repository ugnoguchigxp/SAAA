import { signalSmokeProcess } from "./desktop-smoke-signals";
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";

export type SmokeOptions = {
  root: string;
  reportDir: string;
  build: string[];
  executable: string[];
  verifyBundle?: () => Promise<void>;
  readyTimeoutMs?: number;
  buildTimeoutMs?: number;
};

type Stage = "build" | "bundle" | "launch" | "ready" | "cleanup";
const LOG_LIMIT = 256_000;

export function sanitizeSmokeLog(text: string, root: string): string {
  const values = Object.entries(process.env)
    .filter(([key, value]) => value && /(?:TOKEN|API_KEY|PASSWORD|SECRET|CREDENTIAL)/i.test(key))
    .map(([, value]) => value!)
    .sort((left, right) => right.length - left.length);
  for (const value of values) text = text.replaceAll(value, "[REDACTED]");
  return text
    .replaceAll(root, "[WORKSPACE]")
    .replaceAll(homedir(), "[HOME]")
    .replace(/https?:\/\/[^\s<>"']+/g, "[URL]");
}

function startProcess(command: string[], root: string, env = process.env) {
  const child = spawn(command[0], command.slice(1), {
    cwd: root,
    env,
    detached: process.platform !== "win32",
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
  const kill = (signal: NodeJS.Signals) => signalSmokeProcess(child, signal);
  return { child, exited, closed, kill, events, logs: () => ({ stdout, stderr }) };
}

type SmokeProcess = ReturnType<typeof startProcess>;
async function terminate(child: SmokeProcess): Promise<void> {
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

export async function runDesktopSmoke(options: SmokeOptions): Promise<void> {
  mkdirSync(options.reportDir, { recursive: true });
  const started = Date.now();
  const stages: { stage: Stage; durationMs: number; exitCode?: number }[] = [];
  let stage: Stage = "build";
  let stageStarted = started;
  let application: SmokeProcess | undefined;
  let active: SmokeProcess | undefined;
  let interrupted = false;
  let stageExitCode: number | undefined;
  const interrupt = () => {
    interrupted = true;
    active?.kill("SIGKILL");
  };
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", interrupt);
  let output: Promise<void> | undefined;
  let scratch: string | undefined;
  let failure: string | undefined;
  const advance = (next: Stage, exitCode?: number) => {
    stages.push({
      stage,
      durationMs: Date.now() - stageStarted,
      ...(exitCode === undefined ? {} : { exitCode }),
    });
    stage = next;
    stageExitCode = undefined;
    stageStarted = Date.now();
    console.log(`desktop smoke: ${next}`);
  };
  const saveOutput = async (name: string, child: SmokeProcess) => {
    await child.closed;
    const { stdout, stderr } = child.logs();
    writeFileSync(
      join(options.reportDir, `${name}.events.json`),
      JSON.stringify(child.events, null, 2) + "\n",
    );
    writeFileSync(
      join(options.reportDir, `${name}.stdout.log`),
      sanitizeSmokeLog(stdout, options.root),
    );
    writeFileSync(
      join(options.reportDir, `${name}.stderr.log`),
      sanitizeSmokeLog(stderr, options.root),
    );
  };
  try {
    console.log("desktop smoke: build");
    const build = startProcess(options.build, options.root);
    active = build;
    const buildOutput = saveOutput("build", build);
    let timedOut = false;
    const timeout = setTimeout(
      () => {
        timedOut = true;
        build.kill("SIGKILL");
      },
      options.buildTimeoutMs ?? 30 * 60_000,
    );
    let code: number;
    try {
      code = await build.exited;
    } finally {
      try {
        try {
          await terminate(build);
        } finally {
          await buildOutput;
        }
      } finally {
        clearTimeout(timeout);
      }
    }
    active = undefined;
    stageExitCode = code;
    if (interrupted) throw new Error("Desktop smoke interrupted");
    if (timedOut) throw new Error("Desktop build exceeded its timeout");
    if (code !== 0) throw new Error(`Desktop build exited with code ${code}`);
    advance("bundle", code);
    await options.verifyBundle?.();
    if (interrupted) throw new Error("Desktop smoke interrupted");
    advance("launch");
    scratch = mkdtempSync(join(tmpdir(), "saaa-desktop-smoke-"));
    const markerId = `smoke-${crypto.randomUUID()}`;
    const marker = join(tmpdir(), `saaa-frontend-${markerId}.ready`);
    try {
      application = startProcess(options.executable, options.root, {
        ...process.env,
        SAAA_SMOKE_MARKER_ID: markerId,
        SAAA_SMOKE_DATA_DIR: scratch,
        SAAA_SMOKE_EXERCISE_SITUATION: "1",
        ...(process.platform === "darwin" ? { SAAA_SMOKE_REQUIRE_SPEAKER: "1" } : {}),
      });
      active = application;
      // Observe spawn failures immediately; do not leave a rejected exit promise pending.
      let launchError: unknown;
      void application.exited.catch((cause) => {
        launchError = cause;
      });
      output = saveOutput("application", application);
      advance("ready");
      const deadline = Date.now() + (options.readyTimeoutMs ?? 10_000);
      while (!existsSync(marker)) {
        if (interrupted) throw new Error("Desktop smoke interrupted");
        if (launchError) throw launchError;
        if (application.child.exitCode !== null || application.child.signalCode !== null)
          throw new Error(`Desktop exited before ready with code ${application.child.exitCode}`);
        if (Date.now() >= deadline)
          throw new Error("Desktop did not report IPC ready before the deadline");
        await Bun.sleep(25);
      }
      if (interrupted) throw new Error("Desktop smoke interrupted");
      if (application.child.exitCode !== null || application.child.signalCode !== null)
        throw new Error("Desktop exited after reporting ready");
      advance("cleanup");
    } finally {
      try {
        if (application) await terminate(application);
        await output;
      } finally {
        active = undefined;
        rmSync(marker, { force: true });
      }
    }
  } catch (cause) {
    failure = sanitizeSmokeLog(
      cause instanceof Error ? cause.message : String(cause),
      options.root,
    );
  } finally {
    process.removeListener("SIGINT", interrupt);
    process.removeListener("SIGTERM", interrupt);
    if (scratch) rmSync(scratch, { recursive: true, force: true });
    stages.push({
      stage,
      durationMs: Date.now() - stageStarted,
      ...((application?.child.exitCode ?? stageExitCode) === undefined
        ? {}
        : { exitCode: application?.child.exitCode ?? stageExitCode }),
    });
    writeFileSync(
      join(options.reportDir, "summary.json"),
      JSON.stringify(
        {
          status: failure ? "failed" : "ready",
          stage,
          durationMs: Date.now() - started,
          stages,
          ...(failure ? { error: failure } : {}),
        },
        null,
        2,
      ) + "\n",
    );
  }
  if (failure) throw new Error(failure);
}
