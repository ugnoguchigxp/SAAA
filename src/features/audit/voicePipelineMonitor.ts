import type { AuditEvent } from "../../lib/contracts";
import { isHarnessFailureCode } from "../../lib/harnessFailureDiagnostics";
import { projectLfmConversation } from "./lfmConversationMonitor";

export type PipelineStageState =
  | "idle"
  | "waiting"
  | "running"
  | "success"
  | "failure"
  | "blocked"
  | "cancelled"
  | "skipped";

export type PipelineStage = {
  key: "asr" | "llm" | "tts" | "lfm" | "qwen";
  state: PipelineStageState;
  event: AuditEvent | null;
  failureCode: string | null;
};

export type VoicePipelineSnapshot = {
  stages: PipelineStage[];
  anchor: AuditEvent | null;
  sessionId: string | null;
  utteranceId: string | null;
  runId: string | null;
  diagnosis:
    | "lfm-failed"
    | "lfm-running"
    | "lfm-responded"
    | "lfm-reasoning-requested"
    | "no-voice-events"
    | "asr-listening"
    | "asr-stopped"
    | "asr-failed"
    | "delivery-waiting"
    | "delivery-failed"
    | "llm-waiting"
    | "llm-running"
    | "llm-failed"
    | "tts-waiting"
    | "tts-running"
    | "tts-failed"
    | "completed"
    | "completed-without-tts";
  relatedEvents: AuditEvent[];
};

const voiceAnchorNames = new Set(["asr-final-received", "asr-failed", "capture-start-failed"]);

function latest(events: AuditEvent[], predicate: (event: AuditEvent) => boolean) {
  return events.reduce<AuditEvent | null>(
    (found, event) =>
      predicate(event) && (!found || event.sequence > found.sequence) ? event : found,
    null,
  );
}

function stage(
  key: PipelineStage["key"],
  state: PipelineStageState,
  event: AuditEvent | null = null,
): PipelineStage {
  return { key, state, event, failureCode: event?.failureCode ?? null };
}

function terminalLlmEvent(events: AuditEvent[]) {
  return latest(
    events,
    (event) =>
      event.eventName === "runtime-message-completed" ||
      event.eventName === "runtime-failed" ||
      event.eventName === "runtime-cancelled" ||
      event.eventName === "runtime-run-finished",
  );
}

export function projectLatestResponsePipeline(events: AuditEvent[]): VoicePipelineSnapshot {
  const response =
    projectLfmConversation(events, projectStandardResponsePipeline) ??
    projectStandardResponsePipeline(events);
  return projectNewerCaptureSession(events, response) ?? response;
}

function projectNewerCaptureSession(
  events: AuditEvent[],
  response: VoicePipelineSnapshot,
): VoicePipelineSnapshot | null {
  const started = latest(events, (event) => event.eventName === "asr-session-start-requested");
  if (!started || started.sequence <= (response.anchor?.sequence ?? -1)) return null;
  const sessionId = started.sessionId ?? started.correlationId;
  const relatedEvents = events.filter(
    (event) =>
      event.id === started.id ||
      (sessionId !== null && (event.sessionId === sessionId || event.correlationId === sessionId)),
  );
  const current = latest(relatedEvents, () => true) ?? started;
  const stopped = ["asr-stop-finished", "capture-start-cancelled"].includes(current.eventName);
  const failed = current.outcome === "failure" || current.eventName === "capture-start-failed";
  return {
    stages: [
      stage("asr", failed ? "failure" : stopped ? "success" : "running", current),
      stage("llm", "idle"),
      stage("tts", "idle"),
    ],
    anchor: started,
    sessionId,
    utteranceId: null,
    runId: null,
    diagnosis: failed ? "asr-failed" : stopped ? "asr-stopped" : "asr-listening",
    relatedEvents,
  };
}

function projectStandardResponsePipeline(events: AuditEvent[]): VoicePipelineSnapshot {
  const latestAsr = latest(events, (event) => voiceAnchorNames.has(event.eventName));
  const latestTurn = latest(events, (event) => event.eventName === "turn-requested");
  const directTurn =
    latestTurn &&
    (!latestAsr ||
      (latestTurn.sequence > latestAsr.sequence && latestTurn.causationId !== latestAsr.subjectId))
      ? latestTurn
      : null;
  const anchor = directTurn ?? latestAsr;
  if (!anchor) {
    return {
      stages: [stage("asr", "idle"), stage("llm", "idle"), stage("tts", "idle")],
      anchor: null,
      sessionId: null,
      utteranceId: null,
      runId: null,
      diagnosis: "no-voice-events",
      relatedEvents: [],
    };
  }

  const sessionId = directTurn ? null : (anchor.sessionId ?? anchor.correlationId);
  const utteranceId = directTurn ? null : anchor.subjectId;
  const asrFailed = !directTurn && anchor.eventName !== "asr-final-received";
  if (asrFailed) {
    const cancelled = anchor.outcome === "cancelled";
    return {
      stages: [
        stage("asr", cancelled ? "cancelled" : "failure", anchor),
        stage("llm", "idle"),
        stage("tts", "idle"),
      ],
      anchor,
      sessionId,
      utteranceId,
      runId: null,
      diagnosis: "asr-failed",
      relatedEvents: events.filter(
        (event) =>
          event.id === anchor.id ||
          (sessionId !== null &&
            (event.sessionId === sessionId || event.correlationId === sessionId)),
      ),
    };
  }

  const deliveryEvent = latest(
    events,
    (event) =>
      event.subjectId === utteranceId &&
      [
        "voice-utterance-submitted",
        "voice-utterance-queued",
        "voice-delivery-blocked",
        "voice-delivery-finished",
      ].includes(event.eventName),
  );
  const turnRequest =
    directTurn ??
    latest(
      events,
      (event) => event.eventName === "turn-requested" && event.causationId === utteranceId,
    );
  const runId = turnRequest?.runtimeRunId ?? null;
  const runEvents = runId
    ? events.filter((event) => event.runtimeRunId === runId || event.correlationId === runId)
    : [];

  let llm = stage("llm", "waiting", deliveryEvent ?? anchor);
  let diagnosis: VoicePipelineSnapshot["diagnosis"] = "delivery-waiting";
  if (turnRequest) {
    const terminal = terminalLlmEvent(runEvents);
    if (terminal?.eventName === "runtime-message-completed" || terminal?.outcome === "success") {
      llm = stage("llm", "success", terminal);
      diagnosis = "completed";
    } else if (terminal?.outcome === "cancelled" || terminal?.eventName === "runtime-cancelled") {
      llm = stage("llm", "cancelled", terminal);
      diagnosis = "llm-failed";
    } else if (terminal?.outcome === "failure" || terminal?.eventName === "runtime-failed") {
      const lastProviderFailure = latest(
        runEvents,
        (event) => event.eventName === "runtime-provider-failed",
      );
      const providerDiagnostic = isHarnessFailureCode(lastProviderFailure?.failureCode ?? "")
        ? lastProviderFailure
        : null;
      llm = stage("llm", "failure", providerDiagnostic ?? terminal);
      diagnosis = "llm-failed";
    } else {
      const progress = latest(runEvents, () => true) ?? turnRequest;
      llm = stage(
        "llm",
        runEvents.some((event) => event.phase === "start") ? "running" : "waiting",
        progress,
      );
      diagnosis = llm.state === "running" ? "llm-running" : "llm-waiting";
    }
  } else if (deliveryEvent?.outcome === "blocked" || deliveryEvent?.outcome === "failure") {
    llm = stage("llm", "blocked", deliveryEvent);
    diagnosis = "delivery-failed";
  }

  const presentationMode = turnRequest?.attributes.presentationMode;
  let tts = stage("tts", "idle");
  if (llm.state === "success" && presentationMode !== "visual-and-spoken") {
    tts = stage("tts", "skipped", llm.event);
    diagnosis = "completed-without-tts";
  } else if (presentationMode === "visual-and-spoken" && llm.state === "success") {
    const ttsEvent = latest(
      runEvents,
      (event) =>
        event.component === "tts" ||
        ["speech-started", "speech-ended", "speech-failed"].includes(event.eventName),
    );
    if (
      ttsEvent?.eventName === "speech-ended" ||
      (ttsEvent?.outcome === "success" && ttsEvent.phase === "terminal")
    ) {
      tts = stage("tts", "success", ttsEvent);
      diagnosis = "completed";
    } else if (ttsEvent?.eventName === "speech-failed" || ttsEvent?.outcome === "failure") {
      tts = stage("tts", "failure", ttsEvent);
      diagnosis = "tts-failed";
    } else if (ttsEvent) {
      tts = stage("tts", "running", ttsEvent);
      diagnosis = "tts-running";
    } else {
      tts = stage("tts", "waiting", llm.event);
      diagnosis = "tts-waiting";
    }
  } else if (presentationMode === "visual-and-spoken") {
    tts = stage("tts", "waiting", null);
  }

  const relatedEvents = events.filter(
    (event) =>
      event.id === anchor.id ||
      (sessionId !== null &&
        (event.sessionId === sessionId || event.correlationId === sessionId)) ||
      (utteranceId !== null &&
        (event.subjectId === utteranceId || event.causationId === utteranceId)) ||
      (runId !== null && (event.runtimeRunId === runId || event.correlationId === runId)),
  );

  return {
    stages: [stage("asr", directTurn ? "skipped" : "success", anchor), llm, tts],
    anchor,
    sessionId,
    utteranceId,
    runId,
    diagnosis,
    relatedEvents,
  };
}
