import type { AuditEvent } from "../../lib/contracts";
import type { PipelineStage, VoicePipelineSnapshot } from "./voicePipelineMonitor";

/** LFM's current conversation and the independently running Qwen worker are separate lanes. */
export function projectLfmConversation(
  events: AuditEvent[],
  projectWorker: (events: AuditEvent[]) => VoicePipelineSnapshot,
): VoicePipelineSnapshot | null {
  const ordered = [...events].sort((a, b) => b.sequence - a.sequence);
  const received = ordered.find((e) => e.eventName === "lfm-utterance-received");
  if (!received) return null;
  const latestAsr = ordered.find((e) =>
    ["asr-final-received", "asr-failed", "capture-start-failed"].includes(e.eventName),
  );
  if (latestAsr && latestAsr.sequence > received.sequence) return null;
  const reasoningRequests = new Set(
    ordered
      .filter((e) => e.conversationId === received.conversationId)
      .map((e) => e.attributes.reasoningRequestId)
      .filter((id): id is string => typeof id === "string"),
  );
  const newestTurn = ordered.find((e) => e.eventName === "turn-requested");
  if (
    newestTurn &&
    newestTurn.sequence > received.sequence &&
    !reasoningRequests.has(newestTurn.causationId ?? "")
  )
    return null;
  const utterance = received.subjectId;
  const lfmEvents = ordered.filter(
    (e) => e.subjectId === utterance && e.eventName.startsWith("lfm-"),
  );
  const decision = lfmEvents.find((e) =>
    ["lfm-replied-without-delegation", "lfm-delegated-to-qwen", "lfm-response-failed"].includes(
      e.eventName,
    ),
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
        state: decision?.eventName === "lfm-delegated-to-qwen" ? "waiting" : "skipped",
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
    sessionId: latestAsr?.sessionId ?? null,
    utteranceId: utterance,
    runId: turn?.runtimeRunId ?? null,
    stages: [
      { key: "asr", state: "success", event: latestAsr ?? received, failureCode: null },
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
            : (worker?.diagnosis ?? "lfm-delegated"),
    relatedEvents: ordered.filter((e) => e.conversationId === received.conversationId),
  };
}
