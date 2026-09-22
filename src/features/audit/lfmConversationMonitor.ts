import type { AuditEvent } from "../../lib/contracts";
import type { PipelineStage, VoicePipelineSnapshot } from "./voicePipelineMonitor";

/** LFM's current conversation and the independently running Qwen worker are separate lanes. */
export function projectLfmConversation(
  events: AuditEvent[],
  projectWorker: (events: AuditEvent[]) => VoicePipelineSnapshot,
): VoicePipelineSnapshot | null {
  const ordered = [...events].sort((a, b) => b.sequence - a.sequence);
  const received = ordered.find((e) =>
    ["lfm-utterance-received", "lfm-utterance-rejected"].includes(e.eventName),
  );
  if (!received) return null;
  const newestAsr = ordered.find((e) =>
    ["asr-final-received", "asr-failed", "capture-start-failed"].includes(e.eventName),
  );
  if (newestAsr && newestAsr.sequence > received.sequence) return null;
  const matchingAsr = ordered.find(
    (e) =>
      e.subjectId === received.subjectId &&
      ["asr-final-received", "asr-failed", "capture-start-failed"].includes(e.eventName),
  );
  const utterance = received.subjectId;
  const lfmEvents = ordered.filter(
    (e) => e.subjectId === utterance && e.eventName.startsWith("lfm-"),
  );
  const reasoningRequests = new Set(
    lfmEvents
      .map((e) => e.attributes.reasoningRequestId ?? e.attributes.handoffId)
      .filter((id): id is string => typeof id === "string"),
  );
  const newestTurn = ordered.find((e) => e.eventName === "turn-requested");
  if (
    newestTurn &&
    newestTurn.sequence > received.sequence &&
    !reasoningRequests.has(newestTurn.causationId ?? "")
  )
    return null;
  const decision = lfmEvents.find((e) =>
    [
      "lfm-replied-without-reasoning-request",
      "lfm-silent-without-reasoning-request",
      "lfm-requested-qwen-reasoning",
      "lfm-replied-without-delegation",
      "lfm-delegated-to-qwen",
      "lfm-response-failed",
      "lfm-utterance-rejected",
    ].includes(e.eventName),
  );
  const lfm: PipelineStage = {
    key: "lfm",
    state: decision ? (decision.failureCode ? "failure" : "success") : "running",
    event: decision ?? lfmEvents[0] ?? received,
    failureCode: decision?.failureCode ?? null,
  };
  const turn = ordered.find(
    (e) => e.eventName === "turn-requested" && reasoningRequests.has(e.causationId ?? ""),
  );
  const worker = turn
    ? projectWorker(
        events.filter(
          (e) =>
            e.id === turn.id ||
            e.runtimeRunId === turn.runtimeRunId ||
            e.correlationId === turn.runtimeRunId,
        ),
      )
    : null;
  const qwen: PipelineStage = worker
    ? { ...worker.stages[1]!, key: "qwen" }
    : {
        key: "qwen",
        state: ["lfm-requested-qwen-reasoning", "lfm-delegated-to-qwen"].includes(
          decision?.eventName ?? "",
        )
          ? "waiting"
          : "skipped",
        event: null,
        failureCode: null,
      };
  const speechEvent = lfmEvents.find((e) => e.eventName.startsWith("lfm-speech-"));
  const tts: PipelineStage =
    worker && ["running", "waiting", "failure"].includes(worker.stages[2]!.state)
      ? worker.stages[2]!
      : {
          key: "tts",
          state: speechEvent?.eventName === "lfm-speech-queued" ? "waiting" : "idle",
          event: speechEvent ?? null,
          failureCode: speechEvent?.failureCode ?? null,
        };
  return {
    anchor: received,
    sessionId: matchingAsr?.sessionId ?? null,
    utteranceId: utterance,
    runId: turn?.runtimeRunId ?? null,
    stages: [
      { key: "asr", state: "success", event: matchingAsr ?? received, failureCode: null },
      lfm,
      qwen,
      tts,
    ],
    diagnosis:
      lfm.state === "failure"
        ? "lfm-failed"
        : lfm.state === "running"
          ? "lfm-running"
          : qwen.state === "skipped"
            ? "lfm-responded"
            : (worker?.diagnosis ?? "lfm-reasoning-requested"),
    relatedEvents: ordered.filter((e) => e.conversationId === received.conversationId),
  };
}
