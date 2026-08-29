import { Database } from "bun:sqlite";
import {
  OPTIONAL_RUNTIME_IDS,
  RESIDENT_DEFAULT_RUNTIME_IDS,
  RunnerError,
  emptyReport,
  reportFailure,
  type ReadinessReport,
} from "./schema.ts";
import { type ValidatedEnvironment } from "./io.ts";
import {
  applicationDetectedForbiddenData,
  startApplication,
  stopApplication,
  type OwnedApplication,
} from "./process.ts";
import {
  databaseObservation,
  databaseSnapshot,
  openCanaryDatabase,
  settingsState,
  type DatabaseObservation,
  type DatabaseSnapshot,
  type ProviderSessionRow,
  type SettingsState,
} from "./database.ts";
import { fixedProgress, runRustLiveSuite } from "./bundle.ts";

export async function waitForCheckpoint(
  application: OwnedApplication,
  code: string,
  predicate: () => boolean,
  deadlineAt: number,
): Promise<void> {
  fixedProgress(code);
  const checkpointDeadline = Math.min(deadlineAt, performance.now() + 5 * 60_000);
  while (performance.now() < checkpointDeadline) {
    if (application.child.exitCode !== null || applicationDetectedForbiddenData(application)) {
      throw new RunnerError(2, applicationDetectedForbiddenData(application) ? "redaction-failed" : "restart-recovery-failed", "failed");
    }
    if (predicate()) return;
    await Bun.sleep(1_000);
  }
  throw new RunnerError(3, "gate-missing", "blocked");
}

export function withCanaryDatabase<T>(dataDirectory: string, operation: (database: Database) => T): T {
  const database = openCanaryDatabase(dataDirectory);
  try {
    return operation(database);
  } finally {
    database.close();
  }
}

export function observeDatabase(environment: ValidatedEnvironment, snapshot: DatabaseSnapshot): DatabaseObservation {
  return withCanaryDatabase(environment.dataDirectory, (database) => databaseObservation(database, snapshot));
}

export function observeSettings(environment: ValidatedEnvironment): SettingsState {
  return withCanaryDatabase(environment.dataDirectory, (database) => settingsState(database, environment.manifest));
}

export function assertLarmPrimarySettings(environment: ValidatedEnvironment, larmId: string): void {
  const settings = observeSettings(environment);
  if (!settings.larmPrimary || settings.larmId !== larmId) {
    throw new RunnerError(2, "rollback-failed", "failed");
  }
}

export function countSessions(
  observation: DatabaseObservation,
  predicate: (session: ProviderSessionRow) => boolean,
): number {
  return observation.sessions.filter(predicate).length;
}

export function knownObservationIdentifiers(observation: DatabaseObservation): string[] {
  return [
    ...observation.runs.flatMap((run) => [run.id, run.conversation_id]),
    ...observation.sessions.flatMap((session) => [
      session.id,
      session.runtime_run_id,
      session.allocation_id,
      session.selected_runtime_id,
      session.request_id,
    ].filter((value): value is string => value !== null)),
  ];
}

export function knownDatabaseIdentifiersOrEmpty(
  environment: ValidatedEnvironment,
  snapshot: DatabaseSnapshot,
): string[] {
  try {
    return knownObservationIdentifiers(observeDatabase(environment, snapshot));
  } catch {
    return [];
  }
}

export function runtimeCategory(runtimeId: string): "resident-default" | "optional" | "unknown" {
  if (RESIDENT_DEFAULT_RUNTIME_IDS.has(runtimeId)) return "resident-default";
  return OPTIONAL_RUNTIME_IDS.has(runtimeId) ? "optional" : "unknown";
}

export function validateFunctionalObservation(
  observation: DatabaseObservation,
  larmId: string,
): void {
  const larm = observation.sessions.filter((session) => session.provider_id === larmId && session.provider_kind === "larm");
  const direct = observation.sessions.filter((session) => session.provider_id === "local-openai-compatible" && session.provider_kind === "openai-compatible");
  const failureKinds = larm.filter((session) => session.status === "failed").map((session) => session.failure_kind).sort();
  const failureByKind = (kind: string) => larm.find((session) => session.status === "failed" && session.failure_kind === kind);
  const statusCounts = (status: string) => observation.runs.filter((run) => run.status === status).length;
  const allocationIds = larm.flatMap((session) => session.allocation_id === null ? [] : [session.allocation_id]);
  const selectedRuntimes = larm.flatMap((session) => session.selected_runtime_id === null ? [] : [session.selected_runtime_id]);
  const runIds = new Set(observation.runs.map((run) => run.id));
  const releaseInvalid = larm.some((session) => session.allocation_id !== null && !["released", "deferred-to-ttl"].includes(session.release_status));
  const implicitFallback = larm.some((session) => session.fallback_used !== 0);
  const runtimePolicyInvalid = larm.some((session) => {
    if (session.selected_runtime_id === null) {
      return session.failure_kind !== "capacity"
        || session.allocation_id !== null
        || session.route_id !== null
        || session.selection_reason !== null
        || session.release_status !== "not-started";
    }
    return session.allocation_id === null
      || session.route_id !== "llm-default"
      || session.selection_reason !== "primary";
  });
  if (
    implicitFallback
    || runtimePolicyInvalid
    || selectedRuntimes.some((runtime) => runtimeCategory(runtime) !== "resident-default")
  ) {
    throw new RunnerError(2, "runtime-policy-violation", "failed");
  }
  if (new Set(allocationIds).size !== allocationIds.length) {
    throw new RunnerError(2, "runtime-policy-violation", "failed");
  }
  if (releaseInvalid) throw new RunnerError(2, "allocation-leak", "failed");
  if (
    observation.runs.length !== 14
    || observation.sessions.length !== 16
    || observation.sessions.some((session) => session.runtime_run_id === null || !runIds.has(session.runtime_run_id))
    || observation.runs.some((run) => !observation.sessions.some((session) => session.runtime_run_id === run.id))
    || statusCounts("completed") !== 12
    || statusCounts("cancelled") !== 1
    || statusCounts("failed") !== 1
    || larm.length !== 7
    || direct.length !== 9
    || larm.filter((session) => session.status === "completed").length !== 3
    || larm.filter((session) => session.status === "cancelled").length !== 1
    || direct.some((session) => session.status !== "completed"
      || session.failure_kind !== null
      || session.fallback_used !== 0
      || session.output_started !== 1
      || session.allocation_id !== null
      || session.selected_runtime_id !== null
      || session.request_id !== null
      || session.route_id !== null
      || session.selection_reason !== null
      || session.release_status !== "not-applicable")
    || failureKinds.join(",") !== "capacity,partial-output,timeout"
    || failureByKind("partial-output")?.output_started !== 1
    || failureByKind("timeout")?.output_started !== 0
    || failureByKind("capacity")?.output_started !== 0
    || larm.some((session) => session.status === "completed"
      ? session.failure_kind !== null || session.output_started !== 1
      : session.status === "cancelled"
        ? session.failure_kind !== "cancelled" || session.output_started !== 1
        : false)
    || observation.runs.some((run) => {
      const sessions = observation.sessions.filter((session) => session.runtime_run_id === run.id);
      if (run.status === "completed") {
        return !(
          (sessions.length === 1 && sessions[0]!.status === "completed")
          || (sessions.length === 2
            && sessions.some((session) => session.provider_id === larmId
              && session.status === "failed"
              && ["capacity", "timeout"].includes(session.failure_kind ?? ""))
            && sessions.some((session) => session.provider_id === "local-openai-compatible" && session.status === "completed"))
        );
      }
      if (run.status === "cancelled") {
        return sessions.length !== 1
          || sessions[0]!.provider_id !== larmId
          || sessions[0]!.status !== "cancelled";
      }
      if (run.status === "failed") {
        return sessions.length !== 1
          || sessions[0]!.provider_id !== larmId
          || sessions[0]!.status !== "failed"
          || sessions[0]!.failure_kind !== "partial-output";
      }
      return true;
    })
  ) {
    throw new RunnerError(2, "database-schema-invalid", "failed");
  }
  const explicitFallbacks = observation.runs.filter((run) => {
    const sessions = observation.sessions.filter((session) => session.runtime_run_id === run.id);
    return sessions.some((session) => session.provider_id === larmId && ["capacity", "timeout"].includes(session.failure_kind ?? ""))
      && sessions.some((session) => session.provider_id === "local-openai-compatible" && session.status === "completed");
  }).length;
  if (explicitFallbacks !== 2) throw new RunnerError(2, "rollback-failed", "failed");
}

export async function observeFunctional(
  environment: ValidatedEnvironment,
  identity: { artifactSha256: string; saaaCommit: string },
  deadlineAt: number,
): Promise<{ local: ReadinessReport; rust: ReadinessReport }> {
  let application: OwnedApplication | undefined = await startApplication(environment, false);
  let snapshot: DatabaseSnapshot;
  let primaryError: unknown;
  try {
    snapshot = withCanaryDatabase(environment.dataDirectory, databaseSnapshot);
    await waitForCheckpoint(application, "waiting-for-isolated-settings", () => {
      const settings = observeSettings(environment);
      const observation = observeDatabase(environment, snapshot);
      return settings.directPrimary
        && countSessions(observation, (session) => session.provider_id === "local-openai-compatible" && session.status === "completed") >= 1;
    }, deadlineAt);
    await waitForCheckpoint(application, "enabling-larm-canary", () => observeSettings(environment).larmPrimary, deadlineAt);
    await stopApplication(application, knownDatabaseIdentifiersOrEmpty(environment, snapshot));
    application = undefined;

    application = await startApplication(environment, true);
    const larmId = observeSettings(environment).larmId;
    await waitForCheckpoint(application, "waiting-for-ui-workload", () => {
      const observation = observeDatabase(environment, snapshot);
      return countSessions(observation, (session) => session.provider_id === larmId && session.status === "completed") >= 1
        && countSessions(observation, (session) => session.provider_id === larmId && session.status === "cancelled") >= 1;
    }, deadlineAt);
    await waitForCheckpoint(application, "checkpoint-timeout-fixture-ready", () => {
      const observation = observeDatabase(environment, snapshot);
      return countSessions(observation, (session) => session.provider_id === larmId && session.failure_kind === "timeout") >= 1
        && countSessions(observation, (session) => session.provider_id === "local-openai-compatible" && session.status === "completed") >= 2;
    }, deadlineAt);
    await waitForCheckpoint(application, "checkpoint-tunnel-interruption-ready", () => {
      const observation = observeDatabase(environment, snapshot);
      return countSessions(observation, (session) => session.provider_id === larmId && session.failure_kind === "partial-output") >= 1
        && countSessions(observation, (session) => session.provider_id === larmId && session.status === "completed") >= 2;
    }, deadlineAt);
    await waitForCheckpoint(application, "checkpoint-larm-restart-ready", () => {
      const observation = observeDatabase(environment, snapshot);
      return countSessions(observation, (session) => session.provider_id === larmId && session.status === "completed") >= 3;
    }, deadlineAt);
    await waitForCheckpoint(application, "checkpoint-capacity-fixture-ready", () => {
      const observation = observeDatabase(environment, snapshot);
      return countSessions(observation, (session) => session.provider_id === larmId && session.failure_kind === "capacity") >= 1
        && countSessions(observation, (session) => session.provider_id === "local-openai-compatible" && session.status === "completed") >= 3;
    }, deadlineAt);

    const rust = await runRustLiveSuite("functional", environment, identity, deadlineAt);
    const rustFailure = reportFailure(rust);
    if (rustFailure !== undefined) throw rustFailure;
    await waitForCheckpoint(application, "waiting-for-ui-workload", () => {
      const settings = observeSettings(environment);
      const observation = observeDatabase(environment, snapshot);
      return settings.directPrimary
        && countSessions(observation, (session) => session.provider_id === "local-openai-compatible" && session.status === "completed") >= 6;
    }, deadlineAt);
    await waitForCheckpoint(application, "enabling-larm-canary", () => observeSettings(environment).larmPrimary, deadlineAt);
    await stopApplication(application, knownDatabaseIdentifiersOrEmpty(environment, snapshot));
    application = undefined;

    fixedProgress("checkpoint-saaa-restart");
    application = await startApplication(environment, false);
    await waitForCheckpoint(application, "waiting-for-ui-workload", () => {
      const observation = observeDatabase(environment, snapshot);
      return countSessions(observation, (session) => session.provider_id === "local-openai-compatible" && session.status === "completed") >= 9;
    }, deadlineAt);
    await stopApplication(application, knownDatabaseIdentifiersOrEmpty(environment, snapshot));
    application = undefined;

    const observation = observeDatabase(environment, snapshot);
    validateFunctionalObservation(observation, larmId);
    const local = emptyReport({
      saaaCommit: identity.saaaCommit,
      artifactSha256: identity.artifactSha256,
      manifestSha256: environment.manifestSha256,
      larmContractCommit: environment.deployedCommit,
      deploymentRevision: environment.deploymentRevision,
    }, "functional");
    Object.assign(local.scenarioCounts, {
      normalTurns: 3,
      cancellations: 1,
      requestTimeouts: 1,
      partialInterruptions: 1,
      larmRestarts: 1,
      saaaRestarts: 1,
      capacityRejections: 1,
      rollbackPreflightTurns: 1,
      settingsRollbackTurns: 3,
      killSwitchRollbackTurns: 3,
    });
    Object.assign(local.resultCounts, {
      completed: 12,
      cancelled: 1,
      expectedFailures: 3,
      explicitProviderFallbacks: 2,
    });
    return { local, rust };
  } catch (error) {
    primaryError = error;
    throw error;
  } finally {
    if (application !== undefined) {
      try {
        await stopApplication(application, knownDatabaseIdentifiersOrEmpty(environment, snapshot));
      } catch (cleanupError) {
        if (
          primaryError === undefined
          || (cleanupError instanceof RunnerError
            && cleanupError.errorCode === "redaction-failed"
            && (!(primaryError instanceof RunnerError) || primaryError.errorCode !== "redaction-failed"))
        ) {
          throw cleanupError;
        }
      }
    }
  }
}

export async function readStreamBytes(stream: ReadableStream<Uint8Array>, limit: number): Promise<Buffer> {
  const reader = stream.getReader();
  const chunks: Buffer[] = [];
  let size = 0;
  try {
    while (true) {
      const next = await reader.read();
      if (next.done) break;
      size += next.value.byteLength;
      if (size > limit) throw new RunnerError(2, "rss-growth", "failed");
      chunks.push(Buffer.from(next.value));
    }
  } finally {
    reader.releaseLock();
  }
  return Buffer.concat(chunks);
}

export async function sampleRssKiB(pid: number): Promise<number> {
  const child = Bun.spawn(["/bin/ps", "-o", "rss=", "-p", String(pid)], {
    cwd: ROOT,
    env: {},
    stdout: "pipe",
    stderr: "pipe",
  });
  const stdout = readStreamBytes(child.stdout, 64);
  const stderr = readStreamBytes(child.stderr, 64);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      void terminateOwnedChild(child).finally(() => reject(new RunnerError(2, "sampling-gap", "failed")));
    }, 2_000);
  });
  try {
    let exitCode: number;
    try {
      exitCode = await Promise.race([child.exited, deadline]);
    } catch (error) {
      await Promise.allSettled([stdout, stderr]);
      throw error;
    }
    const [output, error] = await Promise.all([stdout, stderr]);
    const value = output.toString("ascii").trim();
    if (exitCode !== 0 || error.length !== 0 || !/^\d{1,16}$/.test(value)) {
      throw new RunnerError(2, "rss-growth", "failed");
    }
    const rssKiB = Number(value);
    if (!Number.isSafeInteger(rssKiB) || rssKiB < 0 || rssKiB > 1_073_741_824) {
      throw new RunnerError(2, "rss-growth", "failed");
    }
    return rssKiB;
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

export function median(values: number[]): number {
  if (values.length === 0) throw new RunnerError(2, "sampling-gap", "failed");
  const ordered = [...values].sort((left, right) => left - right);
  const middle = Math.floor(ordered.length / 2);
  return ordered.length % 2 === 0 ? (ordered[middle - 1]! + ordered[middle]!) / 2 : ordered[middle]!;
}

export function validateSoakObservation(observation: DatabaseObservation, larmId: string): { completed: number; cancelled: number } {
  const completed = observation.runs.filter((run) => run.status === "completed").length;
  const cancelled = observation.runs.filter((run) => run.status === "cancelled").length;
  const sessions = observation.sessions.filter((session) => session.provider_id === larmId && session.provider_kind === "larm");
  const allocationIds = sessions.flatMap((session) => session.allocation_id === null ? [] : [session.allocation_id]);
  const runIds = new Set(observation.runs.map((run) => run.id));
  const sessionRunIds = sessions.flatMap((session) => session.runtime_run_id === null ? [] : [session.runtime_run_id]);
  if (
    completed + cancelled !== observation.runs.length
    || sessions.length !== observation.runs.length
    || observation.runs.some((run) => run.provider_id !== larmId)
    || sessionRunIds.length !== sessions.length
    || new Set(sessionRunIds).size !== sessionRunIds.length
    || sessionRunIds.some((runId) => !runIds.has(runId))
    || sessions.some((session) => {
      const run = observation.runs.find((candidate) => candidate.id === session.runtime_run_id);
      return run === undefined || run.status !== session.status;
    })
    || sessions.some((session) => !["completed", "cancelled"].includes(session.status))
    || sessions.some((session) => session.fallback_used !== 0
      || session.output_started !== 1
      || (session.status === "completed" ? session.failure_kind !== null : session.failure_kind !== "cancelled"))
    || sessions.some((session) => session.selected_runtime_id === null
      || runtimeCategory(session.selected_runtime_id) !== "resident-default")
    || sessions.some((session) => session.route_id !== "llm-default" || session.selection_reason !== "primary")
    || sessions.some((session) => session.allocation_id === null || !["released", "deferred-to-ttl"].includes(session.release_status))
    || new Set(allocationIds).size !== allocationIds.length
  ) {
    throw new RunnerError(2, "runtime-policy-violation", "failed");
  }
  return { completed, cancelled };
}

export async function observeSoak(
  mode: "soak-30m" | "soak-2h",
  environment: ValidatedEnvironment,
  identity: { artifactSha256: string; saaaCommit: string },
  deadlineAt: number,
): Promise<{ local: ReadinessReport; rust: ReadinessReport }> {
  const durationMs = mode === "soak-30m" ? 30 * 60_000 : 2 * 60 * 60_000;
  const minimumNormal = mode === "soak-30m" ? 20 : 60;
  const minimumCancelled = mode === "soak-30m" ? 5 : 10;
  const snapshot = withCanaryDatabase(environment.dataDirectory, databaseSnapshot);
  const settings = observeSettings(environment);
  if (!settings.larmPrimary) throw new RunnerError(3, "gate-missing", "blocked");
  const larmId = settings.larmId;
  let application: OwnedApplication | undefined = await startApplication(environment, true);
  const abort = new AbortController();
  const rustPromise = runRustLiveSuite(mode, environment, identity, deadlineAt, abort.signal);
  let rustFailure: unknown;
  void rustPromise.then(
    (report) => {
      rustFailure = reportFailure(report);
    },
    (error: unknown) => {
      rustFailure = error;
    },
  );
  const started = performance.now();
  let nextSampleAt = started;
  let lastRssAt = started;
  let lastDatabaseAt = started;
  let maximumSamplingGapMs = 0;
  let restartCheckpointEmitted = false;
  let localRestartCompleted = false;
  const firstProcessRss: Array<{ elapsedMs: number; rssKiB: number }> = [];
  let primaryError: unknown;
  fixedProgress("waiting-for-ui-workload");
  try {
    while (performance.now() - started < durationMs) {
      if (rustFailure !== undefined) throw rustFailure;
      const now = performance.now();
      if (now < nextSampleAt) await Bun.sleep(nextSampleAt - now);
      const elapsedMs = performance.now() - started;
      if (application.child.exitCode !== null || applicationDetectedForbiddenData(application)) {
        throw new RunnerError(2, applicationDetectedForbiddenData(application) ? "redaction-failed" : "restart-recovery-failed", "failed");
      }
      const rssKiB = await sampleRssKiB(application.child.pid);
      const rssAt = performance.now();
      maximumSamplingGapMs = Math.max(maximumSamplingGapMs, rssAt - lastRssAt);
      lastRssAt = rssAt;
      const observation = observeDatabase(environment, snapshot);
      const databaseAt = performance.now();
      maximumSamplingGapMs = Math.max(maximumSamplingGapMs, databaseAt - lastDatabaseAt);
      lastDatabaseAt = databaseAt;
      if (!localRestartCompleted) firstProcessRss.push({ elapsedMs, rssKiB });
      if (mode === "soak-2h" && !restartCheckpointEmitted && elapsedMs >= 30 * 60_000) {
        if (observation.runs.some((run) => run.status === "running")) {
          nextSampleAt += 5_000;
          continue;
        }
        fixedProgress("checkpoint-larm-restart-ready");
        restartCheckpointEmitted = true;
      }
      if (mode === "soak-2h" && !localRestartCompleted && elapsedMs >= 70 * 60_000) {
        if (observation.runs.some((run) => run.status === "running")) {
          nextSampleAt += 5_000;
          continue;
        }
        fixedProgress("checkpoint-saaa-restart");
        await stopApplication(application, knownObservationIdentifiers(observation));
        application = undefined;
        application = await startApplication(environment, true);
        assertLarmPrimarySettings(environment, larmId);
        localRestartCompleted = true;
        const restartedAt = performance.now();
        maximumSamplingGapMs = Math.max(
          maximumSamplingGapMs,
          restartedAt - lastRssAt,
          restartedAt - lastDatabaseAt,
        );
        lastRssAt = restartedAt;
        lastDatabaseAt = restartedAt;
      }
      nextSampleAt += 5_000;
      if (nextSampleAt < performance.now() - 15_000) throw new RunnerError(2, "sampling-gap", "failed");
    }
    if (mode === "soak-2h" && (!restartCheckpointEmitted || !localRestartCompleted)) {
      throw new RunnerError(2, "restart-recovery-failed", "failed");
    }
    let observation = observeDatabase(environment, snapshot);
    assertLarmPrimarySettings(environment, larmId);
    if (observation.runs.some((run) => run.status === "running")) {
      throw new RunnerError(2, "database-schema-invalid", "failed");
    }
    await stopApplication(application, knownObservationIdentifiers(observation));
    application = undefined;
    observation = observeDatabase(environment, snapshot);
    if (observation.runs.some((run) => run.status === "running")) throw new RunnerError(2, "database-schema-invalid", "failed");
    const workload = validateSoakObservation(observation, larmId);
    if (workload.completed < minimumNormal || workload.cancelled < minimumCancelled) {
      throw new RunnerError(2, "report-schema-invalid", "failed");
    }
    const memoryStart = 10 * 60_000;
    const memoryEnd = mode === "soak-30m" ? 30 * 60_000 : 70 * 60_000;
    const memory = firstProcessRss.filter((sample) => sample.elapsedMs >= memoryStart && sample.elapsedMs <= memoryEnd);
    if (memory.length === 0) throw new RunnerError(2, "sampling-gap", "failed");
    const rssRangeMiB = Math.ceil((Math.max(...memory.map((sample) => sample.rssKiB)) - Math.min(...memory.map((sample) => sample.rssKiB))) / 1_024);
    let previousMedianMiB = 0;
    let lastMedianMiB = 0;
    if (mode === "soak-2h") {
      previousMedianMiB = Math.ceil(median(firstProcessRss.filter((sample) => sample.elapsedMs >= 10 * 60_000 && sample.elapsedMs < 40 * 60_000).map((sample) => sample.rssKiB)) / 1_024);
      lastMedianMiB = Math.ceil(median(firstProcessRss.filter((sample) => sample.elapsedMs >= 40 * 60_000 && sample.elapsedMs <= 70 * 60_000).map((sample) => sample.rssKiB)) / 1_024);
    }
    const local = emptyReport({
      saaaCommit: identity.saaaCommit,
      artifactSha256: identity.artifactSha256,
      manifestSha256: environment.manifestSha256,
      larmContractCommit: environment.deployedCommit,
      deploymentRevision: environment.deploymentRevision,
    }, mode);
    local.scenarioCounts.normalTurns = workload.completed;
    local.scenarioCounts.cancellations = workload.cancelled;
    local.scenarioCounts.larmRestarts = mode === "soak-2h" ? 1 : 0;
    local.scenarioCounts.saaaRestarts = mode === "soak-2h" ? 1 : 0;
    local.resultCounts.completed = workload.completed;
    local.resultCounts.cancelled = workload.cancelled;
    local.timingSummary.elapsedMs = Math.min(10_800_000, Math.ceil(performance.now() - started));
    local.timingSummary.sampleIntervalSeconds = 5;
    local.timingSummary.rssMaxSamplingGapSeconds = Math.ceil(maximumSamplingGapMs / 1_000);
    local.resourceSummary.rssRangeMiB = rssRangeMiB;
    local.resourceSummary.rssPrevious30mMedianMiB = previousMedianMiB;
    local.resourceSummary.rssLast30mMedianMiB = lastMedianMiB;
    const rust = await rustPromise;
    const completedRustFailure = reportFailure(rust);
    if (completedRustFailure !== undefined) throw completedRustFailure;
    return { local, rust };
  } catch (error) {
    primaryError = error;
    abort.abort();
    await Promise.allSettled([rustPromise]);
    throw error;
  } finally {
    if (application !== undefined) {
      try {
        await stopApplication(application, knownDatabaseIdentifiersOrEmpty(environment, snapshot));
      } catch (cleanupError) {
        if (
          primaryError === undefined
          || (cleanupError instanceof RunnerError
            && cleanupError.errorCode === "redaction-failed"
            && (!(primaryError instanceof RunnerError) || primaryError.errorCode !== "redaction-failed"))
        ) {
          throw cleanupError;
        }
      }
    }
  }
}
