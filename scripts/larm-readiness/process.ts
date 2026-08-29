import { randomBytes } from "node:crypto";
import { existsSync, lstatSync, unlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  MAX_CHILD_BYTES,
  RELEASE_EXECUTABLE,
  ROOT,
  RunnerError,
} from "./schema.ts";
import { ForbiddenDataScanner, appChildEnvironment, type ValidatedEnvironment } from "./io.ts";


export interface ChildResult {
  exitCode: number;
  stdoutBytes: number;
  stderrBytes: number;
  redactionFailed: boolean;
}

export async function consumeBounded(
  stream: ReadableStream<Uint8Array>,
  limit: number,
  scanner: ForbiddenDataScanner,
  onOverflow: () => void,
  onForbidden: () => void,
  onChunk?: (chunk: Uint8Array) => void,
): Promise<number> {
  const reader = stream.getReader();
  let size = 0;
  try {
    while (true) {
      const next = await reader.read();
      if (next.done) break;
      size += next.value.byteLength;
      if (size > limit) {
        onOverflow();
        throw new RunnerError(70, "internal", "failed");
      }
      onChunk?.(next.value);
      if (scanner.scan(next.value)) {
        onForbidden();
        throw new RunnerError(2, "redaction-failed", "failed");
      }
    }
  } finally {
    reader.releaseLock();
  }
  return size;
}

export async function terminateOwnedChild(child: ReturnType<typeof Bun.spawn>): Promise<void> {
  const signalGroup = (signal: NodeJS.Signals): boolean => {
    if (child.pid > 1) {
      try {
        process.kill(-child.pid, signal);
        return true;
      } catch {
        // A non-detached helper has no child-owned process group; signal its exact PID below.
      }
    }
    if (child.exitCode === null) {
      try {
        child.kill(signal);
        return true;
      } catch {
        // The child crossed the exit boundary after the status check.
      }
    }
    return false;
  };
  const groupExists = (): boolean => {
    if (child.pid <= 1) return false;
    try {
      process.kill(-child.pid, 0);
      return true;
    } catch {
      return false;
    }
  };
  if (!signalGroup("SIGTERM")) return;
  const gracefulDeadline = performance.now() + 5_000;
  while (performance.now() < gracefulDeadline && (child.exitCode === null || groupExists())) {
    await Bun.sleep(50);
  }
  if (child.exitCode === null || groupExists()) signalGroup("SIGKILL");
  if (child.exitCode === null) {
    await child.exited;
  }
}

export async function runBoundedChild(options: {
  command: string[];
  environment: Record<string, string>;
  limit: number;
  deadlineMs: number;
  scanner: ForbiddenDataScanner;
  signal?: AbortSignal;
}): Promise<ChildResult> {
  const child = Bun.spawn(options.command, {
    cwd: ROOT,
    env: options.environment,
    stdout: "pipe",
    stderr: "pipe",
    detached: true,
  });
  const abort = () => void terminateOwnedChild(child);
  if (options.signal?.aborted) abort();
  options.signal?.addEventListener("abort", abort, { once: true });
  let overflow = false;
  let redactionFailed = false;
  const stdoutScanner = options.scanner.fork();
  const stderrScanner = options.scanner.fork();
  const stdout = consumeBounded(child.stdout, options.limit, stdoutScanner, () => {
    overflow = true;
    void terminateOwnedChild(child);
  }, () => {
    redactionFailed = true;
    void terminateOwnedChild(child);
  });
  const stderr = consumeBounded(child.stderr, options.limit, stderrScanner, () => {
    overflow = true;
    void terminateOwnedChild(child);
  }, () => {
    redactionFailed = true;
    void terminateOwnedChild(child);
  });
  let deadlineHandle: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<never>((_, reject) => {
    deadlineHandle = setTimeout(() => {
      void terminateOwnedChild(child).finally(() => reject(new RunnerError(2, "runner-timeout", "failed")));
    }, options.deadlineMs);
  });
  let exitCode: number;
  try {
    exitCode = await Promise.race([child.exited, deadline]);
  } catch (error) {
    await Promise.allSettled([stdout, stderr]);
    throw error;
  } finally {
    if (deadlineHandle !== undefined) clearTimeout(deadlineHandle);
    options.signal?.removeEventListener("abort", abort);
  }
  await terminateOwnedChild(child);
  const streams = await Promise.allSettled([stdout, stderr]);
  if (redactionFailed) throw new RunnerError(2, "redaction-failed", "failed");
  if (overflow) throw new RunnerError(70, "internal", "failed");
  if (streams.some((stream) => stream.status === "rejected")) throw new RunnerError(70, "internal", "failed");
  const stdoutBytes = streams[0].status === "fulfilled" ? streams[0].value : 0;
  const stderrBytes = streams[1].status === "fulfilled" ? streams[1].value : 0;
  return { exitCode, stdoutBytes, stderrBytes, redactionFailed: stdoutScanner.detected || stderrScanner.detected };
}

export interface OwnedApplication {
  child: ReturnType<typeof Bun.spawn>;
  stdoutScanner: ForbiddenDataScanner;
  stderrScanner: ForbiddenDataScanner;
  stdout: Promise<{ ok: boolean }>;
  stderr: Promise<{ ok: boolean }>;
  stdoutCapture: Buffer[];
  stderrCapture: Buffer[];
}

export function applicationDetectedForbiddenData(application: OwnedApplication): boolean {
  return application.stdoutScanner.detected || application.stderrScanner.detected;
}

export function capturedApplicationStream(
  stream: ReadableStream<Uint8Array>,
  scanner: ForbiddenDataScanner,
  child: ReturnType<typeof Bun.spawn>,
  capture: Buffer[],
): Promise<{ ok: boolean }> {
  return consumeBounded(
    stream,
    MAX_CHILD_BYTES,
    scanner,
    () => void terminateOwnedChild(child),
    () => void terminateOwnedChild(child),
    (chunk) => capture.push(Buffer.from(chunk)),
  ).then(
    () => ({ ok: true }),
    () => ({ ok: false }),
  );
}

export async function startApplication(environment: ValidatedEnvironment, enabled: boolean): Promise<OwnedApplication> {
  const markerId = `canary-${randomBytes(16).toString("hex")}`;
  const marker = join(tmpdir(), `saaa-frontend-${markerId}.ready`);
  if (existsSync(marker)) throw new RunnerError(2, "environment-invalid", "failed");
  const scanner = new ForbiddenDataScanner([
    environment.token,
    environment.rollbackCredential,
    environment.baseUrl,
    environment.manifest.rollbackProvider.endpoint,
    "Reply with exactly: CANARY_OK",
    "List the numbers 1 through 5.",
    "Write one short greeting in Japanese.",
    "Write ten numbered words, one at a time.",
    "Reply with exactly: READY",
  ]);
  const child = Bun.spawn([RELEASE_EXECUTABLE], {
    cwd: ROOT,
    env: appChildEnvironment(process.env, { enabled, markerId, dataDirectory: environment.dataDirectory }),
    stdout: "pipe",
    stderr: "pipe",
    detached: true,
  });
  const stdoutScanner = scanner.fork();
  const stderrScanner = scanner.fork();
  const stdoutCapture: Buffer[] = [];
  const stderrCapture: Buffer[] = [];
  const application: OwnedApplication = {
    child,
    stdoutScanner,
    stderrScanner,
    stdout: capturedApplicationStream(child.stdout, stdoutScanner, child, stdoutCapture),
    stderr: capturedApplicationStream(child.stderr, stderrScanner, child, stderrCapture),
    stdoutCapture,
    stderrCapture,
  };
  const started = performance.now();
  while (performance.now() - started <= 10_000) {
    if (existsSync(marker)) {
      const info = lstatSync(marker);
      if (!info.isFile() || info.isSymbolicLink() || info.nlink !== 1) {
        await stopApplication(application);
        throw new RunnerError(2, "restart-recovery-failed", "failed");
      }
      unlinkSync(marker);
      return application;
    }
    if (child.exitCode !== null || applicationDetectedForbiddenData(application)) break;
    await Bun.sleep(100);
  }
  if (existsSync(marker)) unlinkSync(marker);
  await stopApplication(application);
  throw new RunnerError(2, applicationDetectedForbiddenData(application) ? "redaction-failed" : "restart-recovery-failed", "failed");
}

export async function stopApplication(application: OwnedApplication, knownIdentifiers: string[] = []): Promise<void> {
  await terminateOwnedChild(application.child);
  const [stdout, stderr] = await Promise.all([application.stdout, application.stderr]);
  if (!stdout.ok || !stderr.ok || applicationDetectedForbiddenData(application)) {
    throw new RunnerError(2, applicationDetectedForbiddenData(application) ? "redaction-failed" : "internal", "failed");
  }
  const stdoutIdentifierScanner = new ForbiddenDataScanner(knownIdentifiers);
  const stderrIdentifierScanner = new ForbiddenDataScanner(knownIdentifiers);
  for (const chunk of application.stdoutCapture) stdoutIdentifierScanner.scan(chunk);
  for (const chunk of application.stderrCapture) stderrIdentifierScanner.scan(chunk);
  application.stdoutCapture.length = 0;
  application.stderrCapture.length = 0;
  if (stdoutIdentifierScanner.detected || stderrIdentifierScanner.detected) {
    throw new RunnerError(2, "redaction-failed", "failed");
  }
}
