import { toMessage } from "../../lib/appHelpers";
import { recordAuditEvent } from "../../lib/auditRuntime";
import { MicrophoneCaptureError } from "../../lib/microphone";
import type { FinalVoiceUtterance } from "./voiceFinalDeliveryQueue";

function failureCode(cause: unknown): string {
  if (cause instanceof MicrophoneCaptureError) return cause.code;
  const code = toMessage(cause);
  return [
    "asr-provider-unavailable",
    "asr-session-exists",
    "asr-target-speaker-unavailable",
    "asr-language-not-allowed",
    "asr-cancelled",
    "asr-session-not-found",
  ].includes(code) ? code : "unknown";
}

export function auditCaptureStarted(sessionId: string, conversationId: string | null, state: string) {
  recordAuditEvent({ component: "microphone", eventName: "capture-started", phase: "start", outcome: "success", correlationId: sessionId, conversationId, sessionId, attributes: { state } });
}

export function auditCaptureCancelled(sessionId: string, conversationId: string | null) {
  recordAuditEvent({ component: "microphone", eventName: "capture-start-cancelled", phase: "terminal", outcome: "cancelled", correlationId: sessionId, conversationId, sessionId, failureCode: "stale-session" });
}

export function auditCaptureFailed(sessionId: string, conversationId: string | null, cause: unknown) {
  recordAuditEvent({ component: cause instanceof MicrophoneCaptureError ? "microphone" : "voice-asr", eventName: "capture-start-failed", phase: "error", outcome: "failure", correlationId: sessionId, conversationId, sessionId, failureCode: failureCode(cause) });
}

export function auditCaptureSuspended(sessionId: string | null, conversationId: string | null, reason: string) {
  recordAuditEvent({ component: "microphone", eventName: "capture-suspended", phase: "state", outcome: "success", correlationId: sessionId, conversationId, sessionId, attributes: { reasonCode: reason } });
}

export function auditVoiceDeliveryBlocked(sessionId: string, utteranceId: string, conversationId: string, queueDepth?: number) {
  recordAuditEvent({ component: "conversation", eventName: "voice-delivery-blocked", phase: "decision", outcome: "blocked", correlationId: sessionId, causationId: utteranceId, conversationId, sessionId, subjectId: utteranceId, failureCode: "pending-limit", attributes: queueDepth === undefined ? {} : { queueDepth } });
}

export function auditVoiceDeliveryDecision(utterance: FinalVoiceUtterance, deliveryMode: "queued" | "immediate", queueDepth?: number) {
  recordAuditEvent({ component: "conversation", eventName: deliveryMode === "queued" ? "voice-utterance-queued" : "voice-utterance-submitted", phase: "decision", outcome: "success", correlationId: utterance.sessionId, causationId: utterance.utteranceId, conversationId: utterance.conversationId, sessionId: utterance.sessionId, subjectId: utterance.utteranceId, attributes: { deliveryMode, ...(queueDepth === undefined ? {} : { queueDepth }) } });
}

export function auditVoiceDeliverySettlement(
  utterance: FinalVoiceUtterance,
  settle: (delivered: boolean) => void,
): (delivered: boolean) => void {
  return (delivered) => {
    recordAuditEvent({ component: "conversation", eventName: "voice-delivery-finished", phase: "terminal", outcome: delivered ? "success" : "failure", correlationId: utterance.sessionId, causationId: utterance.utteranceId, conversationId: utterance.conversationId, sessionId: utterance.sessionId, subjectId: utterance.utteranceId, failureCode: delivered ? null : "turn-not-accepted" });
    settle(delivered);
  };
}
