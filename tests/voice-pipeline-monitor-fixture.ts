import type { AuditEvent } from "../src/lib/contracts";

export function audit(sequence: number, eventName: string, values: Partial<AuditEvent> = {}): AuditEvent {
  return {
    sequence,
    id: `audit_${sequence}`,
    occurredAt: String(sequence),
    component: "conversation",
    eventName,
    phase: "state",
    outcome: null,
    correlationId: null,
    causationId: null,
    conversationId: "conversation_1",
    runtimeRunId: null,
    sessionId: null,
    subjectId: null,
    failureCode: null,
    attributes: {},
    ...values,
  };
}

export const recognized = audit(1, "asr-final-received", {
  component: "voice-asr",
  phase: "terminal",
  outcome: "success",
  correlationId: "voice_1",
  sessionId: "voice_1",
  subjectId: "utterance_1",
});

