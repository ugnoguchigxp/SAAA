import {
  startSmokeProcess as startProcess,
  terminateSmokeProcess as terminate,
  type SmokeProcess,
} from "./desktop-smoke-child";
import { readDesktopE2EChecks } from "./desktop-e2e-report";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { startLockedBuild } from "./verification-build";

export type SmokeOptions = {
  root: string;
  reportDir: string;
  build: string[];
  executable: string[];
  verifyBundle?: () => Promise<void>;
  readyTimeoutMs?: number;
  buildTimeoutMs?: number;
  requiredChecks?: string[];
};

type Stage = "build" | "bundle" | "launch" | "ready" | "cleanup";

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

export async function runDesktopSmoke(options: SmokeOptions): Promise<void> {
  mkdirSync(options.reportDir, { recursive: true });
  const started = Date.now();
  const stages: { stage: Stage; durationMs: number; exitCode?: number }[] = [];
  let stage: Stage = "build";
  let stageStarted = started;
  let application: SmokeProcess | undefined;
  let active: SmokeProcess | undefined;
  let interrupted = false;
  const abort = new AbortController();
  let stageExitCode: number | undefined;
  const interrupt = () => {
    interrupted = true;
    abort.abort();
    active?.kill("SIGKILL");
  };
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", interrupt);
  let output: Promise<void> | undefined;
  let scratch: string | undefined;
  let failure: string | undefined;
  let checks: Record<string, boolean> | undefined;
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
    const { build, release } = await startLockedBuild(options.root, abort.signal, (env, detached) =>
      startProcess(options.build, options.root, env, detached),
    );
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
        await release();
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
      if (options.requiredChecks?.length) {
        checks = readDesktopE2EChecks(marker, options.requiredChecks);
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
          ...(checks ? { checks } : {}),
          ...(failure ? { error: failure } : {}),
        },
        null,
        2,
      ) + "\n",
    );
  }
  if (failure) throw new Error(failure);
}
