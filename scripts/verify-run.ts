import { closeSync, mkdtempSync, openSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { verificationPlan, VERIFY_HELP, type VerificationStep } from "./verify-plan";
import { acquireVerificationLock } from "./verification-lock";
import { runVerificationProcess } from "./verification-process";
import {
  explainAffected,
  selectedAllowlistMatches,
  selectedVerificationSteps,
  type AffectedLevel,
  type AffectedMode,
} from "./verify-affected-plan";
import {
  captureInputFingerprint,
  staleDecision,
  type InputFingerprint,
} from "./verify-fingerprint";
import {
  snapshotAttempt,
  writeVerificationReport as saveVerificationReport,
  type AttemptLog,
  type VerificationRecord,
} from "./verify-report";
import { startInputWatch, type InputWatch, type WatchDetail } from "./verify-input-watch";

export const ROOT = fileURLToPath(new URL("..", import.meta.url));

/** File-backed output avoids pipe deadlocks and subprocess output-size limits. */
export type VerificationContext = {
  mode?: "manual" | "shadow" | "selected" | "selected-fallback";
  requestedMode?: "manual" | "shadow" | "selected";
  level?: string | null;
  fallback?: string | null;
  reason?: string | null;
  candidate?: string[];
  requireWatch?: boolean;
  watcherAttached?: boolean;
  frozenFallback?: boolean;
  refuseUnwatched?: boolean;
  scanFailed?: boolean;
};

export async function runVerification(
  steps: VerificationStep[] | (() => VerificationStep[]),
  cwd = ROOT,
  context: VerificationContext = {},
): Promise<number> {
  const plan = typeof steps === "function" ? steps : () => steps;
  const directory = mkdtempSync(join(tmpdir(), "saaa-verify-"));
  const abort = new AbortController();
  let interrupted = 0;
  const interrupt = () => {
    interrupted = 130;
    abort.abort();
  };
  const terminate = () => {
    interrupted = 143;
    abort.abort();
  };
  process.on("SIGINT", interrupt);
  process.on("SIGTERM", terminate);
  let lock: Awaited<ReturnType<typeof acquireVerificationLock>> | undefined;
  const records: VerificationRecord[] = [];
  const attemptLog: AttemptLog[] = [];
  const started = performance.now();
  let watch: InputWatch | undefined;
  try {
    assertReportDestination(process.env.SAAA_VERIFY_REPORT, cwd);
    const lockedAt = performance.now();
    lock = await acquireVerificationLock(cwd, abort.signal);
    const lockWaitMs = Math.round(performance.now() - lockedAt);
    if (context.requireWatch) {
      watch = startInputWatch(cwd);
      const opened = watch.begin();
      context.watcherAttached = watch.available && opened.ok;
      context.scanFailed = !opened.ok;
    }
    // The watcher and the opening scan are already running before the plan is built.
    let fingerprint = captureInputFingerprint(cwd);
    let planned = plan();
    if (refusedNarrow(context)) {
      console.error("selected verification inputs could not be watched");
      writeVerificationReport(
        records,
        2,
        Math.round(performance.now() - started),
        undefined,
        undefined,
        1,
        context,
        attemptLog,
        watch?.detail(),
        cwd,
      );
      return 2;
    }
    for (let attempt = 1; attempt <= 2; attempt += 1) {
      const attemptStarted = performance.now();
      const attemptContext = { ...context, candidate: context.candidate?.slice() };
      const failed = await runVerificationSteps(
        planned,
        cwd,
        directory,
        lock,
        abort.signal,
        records,
        lockWaitMs,
        () => interrupted,
        started,
        attempt,
        attemptStarted,
        attemptContext,
        attemptLog,
        watch,
      );
      if (failed !== null) return failed;
      const narrow = attemptContext.mode === "selected";
      if (narrow) watch?.finalScan();
      const after = captureInputFingerprint(cwd);
      // Deliver queued watch errors/events while the final scan is still under observation.
      if (narrow) await new Promise<void>((done) => setImmediate(done));
      const end = narrow ? watch?.detail() : undefined;
      const decision = staleDecision(fingerprint, after, attempt);
      const watchBroken =
        narrow && (!watch?.available || watch.overflow() || watch.failed() || !end?.scanned);
      const inputChanged =
        decision === "retry" ||
        decision === "stale" ||
        (narrow && (!end?.scanMatched || end.generationStart !== end.generationEnd));
      const elapsed = Math.round(performance.now() - started);
      attemptLog.push(
        snapshotAttempt(
          attempt,
          Math.round(performance.now() - attemptStarted),
          records,
          attemptContext,
          end ?? watch?.detail() ?? null,
        ),
      );
      if (
        watchBroken ||
        (inputChanged && attempt >= 2) ||
        decision === "unmeasured" ||
        decision === "stale"
      ) {
        writeVerificationReport(
          records,
          2,
          elapsed,
          fingerprint,
          after,
          attempt,
          context,
          attemptLog,
          end ?? watch?.detail(),
          cwd,
        );
        console.error(staleReason(decision, narrow, watchBroken));
        return 2;
      }
      if (inputChanged) {
        records.length = 0;
        if (narrow) {
          const opened = watch?.begin();
          context.scanFailed = opened?.ok !== true;
          context.watcherAttached =
            watch?.available === true &&
            !watch.failed() &&
            !watch.overflow() &&
            opened?.ok === true;
        }
        fingerprint = captureInputFingerprint(cwd);
        planned = plan();
        if (refusedNarrow(context)) {
          console.error("selected verification inputs could not be watched");
          writeVerificationReport(
            records,
            2,
            elapsed,
            fingerprint,
            after,
            attempt,
            context,
            attemptLog,
            watch?.detail(),
            cwd,
          );
          return 2;
        }
        continue;
      }
      writeVerificationReport(
        records,
        0,
        elapsed,
        fingerprint,
        after,
        attempt,
        context,
        attemptLog,
        end ?? watch?.detail(),
        cwd,
      );
      console.log("OK");
      return 0;
    }
    return 2;
  } catch (cause) {
    if (interrupted) return interrupted;
    throw cause;
  } finally {
    watch?.close();
    lock?.release();
    process.off("SIGINT", interrupt);
    process.off("SIGTERM", terminate);
    rmSync(directory, { recursive: true, force: true });
  }
}

function refusedNarrow(context: VerificationContext): boolean {
  return (
    context.mode === "selected" &&
    (context.watcherAttached !== true ||
      context.scanFailed === true ||
      context.refuseUnwatched === true)
  );
}

function staleReason(
  decision: ReturnType<typeof staleDecision>,
  narrow: boolean,
  watchBroken: boolean,
): string {
  if (decision === "unmeasured") return "UNMEASURED verification inputs could not be fingerprinted";
  if (watchBroken || narrow)
    return "STALE verification scan did not match the inputs observed at the start";
  return "STALE verification inputs changed while the gate was running";
}

function redactReportText(value: string): string {
  return value
    .replace(/sk-[A-Za-z0-9_-]+/g, "[redacted]")
    .replace(/Bearer\s+\S+/gi, "Bearer [redacted]");
}

async function runVerificationSteps(
  steps: VerificationStep[],
  cwd: string,
  directory: string,
  lock: Awaited<ReturnType<typeof acquireVerificationLock>>,
  signal: AbortSignal,
  records: VerificationRecord[],
  lockWaitMs: number,
  interrupted: () => number,
  started: number,
  attempt: number,
  attemptStarted: number,
  context: VerificationContext,
  attemptLog: AttemptLog[],
  watch?: InputWatch,
): Promise<number | null> {
  for (const [index, step] of steps.entries()) {
    const log = join(directory, `${index}.log`);
    const descriptor = openSync(log, "w");
    let status = 1;
    let error: unknown;
    const stepStarted = performance.now();
    try {
      status = await runVerificationProcess(step.command, cwd, lock, signal, descriptor);
    } catch (cause) {
      error = cause;
    } finally {
      closeSync(descriptor);
    }
    const stop = interrupted();
    records.push({
      name: step.name,
      command: step.command.map(redactReportText),
      status: status !== 0 || stop || error ? "failed" : "passed",
      durationMs: Math.round(performance.now() - stepStarted),
      lockWaitMs,
    });
    if (status !== 0 || stop || error) {
      console.error(`FAIL ${step.name}\n$ ${step.command.join(" ")}`);
      const output = readFileSync(log);
      if (output.length) await Bun.write(Bun.stderr, output);
      if (error) console.error(error);
      const code = stop || (status > 0 ? status : 1);
      console.error(`Exit status: ${code}`);
      attemptLog.push(
        snapshotAttempt(
          attempt,
          Math.round(performance.now() - attemptStarted),
          records,
          context,
          watch?.detail() ?? null,
        ),
      );
      writeVerificationReport(
        records,
        code,
        Math.round(performance.now() - started),
        undefined,
        undefined,
        attempt,
        context,
        attemptLog,
        watch?.detail(),
        cwd,
      );
      return code;
    }
  }
  return null;
}

/** `normal` is the stage-less verify plan. Passing the word `normal` is an unknown stage. */
export function affectedRunPlan(level: AffectedLevel, root = ROOT) {
  return verificationPlan(level === "normal" ? [] : [level], root);
}

function affectedOptions(args: string[]): {
  level: AffectedLevel;
  mode: AffectedMode;
  explain: boolean;
} {
  let level: AffectedLevel = "normal";
  let mode: AffectedMode = "shadow";
  let explain = false;
  for (let index = 1; index < args.length; index += 1) {
    const option = args[index];
    if (option === "--explain") explain = true;
    else if (option === "--level") {
      const value = args[index + 1];
      index += 1;
      if (value !== "normal" && value !== "advance" && value !== "full") {
        throw new Error(`Invalid --level\n${VERIFY_HELP}`);
      }
      level = value;
    } else if (option === "--mode") {
      const value = args[index + 1];
      index += 1;
      if (value !== "shadow" && value !== "selected") {
        throw new Error(`Invalid --mode\n${VERIFY_HELP}`);
      }
      mode = value;
    } else throw new Error(`Unknown option: ${option}\n${VERIFY_HELP}`);
  }
  return { level, mode, explain };
}

function affectedContext(level: AffectedLevel, mode: AffectedMode): VerificationContext {
  const plan = explainAffected(ROOT, level, mode);
  const narrow =
    mode === "selected" &&
    level !== "full" &&
    !plan.fallback &&
    selectedAllowlistMatches(plan.paths);
  return {
    mode: narrow ? "selected" : mode === "selected" ? "selected-fallback" : "shadow",
    level,
    fallback:
      mode === "selected" && level === "full"
        ? "full is not reduced"
        : narrow
          ? null
          : (plan.fallback ??
            (mode === "selected" ? "selected input is outside the avatar allowlist" : null)),
    reason: plan.reason,
    candidate: plan.candidate,
    requireWatch: narrow,
  };
}

/** Optional machine-readable timings. Success stdout stays a single OK. */
export function writeVerificationReport(...args: Parameters<typeof saveVerificationReport>): void {
  assertReportDestination(process.env.SAAA_VERIFY_REPORT, args[9] ?? ROOT);
  saveVerificationReport(...args);
}

function assertReportDestination(path: string | undefined, cwd: string): void {
  if (!path) return;
  const base = realpathSync(cwd);
  const destination = resolve(path);
  const parent = realpathSync(dirname(destination));
  const canonical = join(parent, basename(destination));
  const permitted = join(base, "src-tauri/target/verify-reports") + sep;
  const inside = (value: string) => value === base || value.startsWith(base + sep);
  if (
    (inside(destination) && !destination.startsWith(permitted)) ||
    (inside(canonical) && !canonical.startsWith(permitted))
  )
    throw new Error(
      "verification report must not overwrite source or other inputs; use a destination outside the repository or in src-tauri/target/verify-reports",
    );
  try {
    if (realpathSync(destination) !== canonical)
      throw new Error("verification report must not follow a symlink");
  } catch (cause) {
    if ((cause as NodeJS.ErrnoException).code !== "ENOENT") throw cause;
  }
}

export async function runAffectedCommand(args: string[]): Promise<number> {
  const options = affectedOptions(args);
  if (options.explain) {
    console.log(JSON.stringify(explainAffected(ROOT, options.level, options.mode), null, 2));
    return 0;
  }
  if (options.mode === "selected" && options.level !== "full") {
    const context: VerificationContext = {
      requestedMode: "selected",
      mode: "selected",
      level: options.level,
      requireWatch: true,
      frozenFallback: false,
    };
    return runVerification(() => selectedPlan(context, options.level), ROOT, context);
  }
  const context = affectedContext(options.level, options.mode);
  context.requestedMode = options.mode === "selected" ? "selected" : "shadow";
  context.requireWatch = false;
  return runVerification(() => affectedRunPlan(options.level), ROOT, context);
}

export function selectedPlan(
  context: VerificationContext,
  level: AffectedLevel,
  root = ROOT,
): VerificationStep[] {
  if (context.frozenFallback) {
    context.mode = "selected-fallback";
    context.requireWatch = false;
    context.fallback ??= "selected fallback stays wide for this invocation";
    return affectedRunPlan(level, root);
  }
  const plan = explainAffected(root, level, "selected");
  const decision = decideSelectedExecution({
    level,
    holdFallback: false,
    paths: plan.paths,
    planFallback: plan.fallback,
  });
  context.frozenFallback = decision.holdFallback;
  context.mode = decision.mode;
  context.fallback = decision.fallback;
  context.reason = plan.reason;
  context.candidate = plan.candidate;
  context.requireWatch = decision.watchRequired;
  if (decision.narrowSteps && context.watcherAttached !== true) {
    context.mode = "selected";
    context.refuseUnwatched = true;
    context.fallback = "watcher was not attached before the narrow plan";
    return [];
  }
  return decision.narrowSteps
    ? selectedVerificationSteps(level, root)
    : affectedRunPlan(level, root);
}

export function decideSelectedExecution(input: {
  level: AffectedLevel;
  holdFallback: boolean;
  paths: string[];
  planFallback: string | null;
}): {
  holdFallback: boolean;
  mode: "selected" | "selected-fallback";
  watchRequired: boolean;
  narrowSteps: boolean;
  fallback: string | null;
} {
  const outside = input.planFallback !== null || !selectedAllowlistMatches(input.paths);
  if (input.level === "full" || input.holdFallback || outside) {
    return {
      holdFallback: true,
      mode: "selected-fallback",
      watchRequired: false,
      narrowSteps: false,
      fallback:
        input.level === "full"
          ? "full is not reduced"
          : (input.planFallback ??
            (input.holdFallback
              ? "selected fallback stays wide for this invocation"
              : "selected input is outside the avatar allowlist")),
    };
  }
  return {
    holdFallback: false,
    mode: "selected",
    watchRequired: true,
    narrowSteps: true,
    fallback: null,
  };
}
