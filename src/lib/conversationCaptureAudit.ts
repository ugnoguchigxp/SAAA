import { recordConversationCaptureAuditEvent } from "./auditRuntime";

export function createConversationCaptureAudit(getSessionId: () => string | null) {
  let captureAuditSequence = 0;
  return function auditCapture(
    eventName: string,
    phase: "request" | "start" | "state" | "progress" | "decision" | "terminal" | "error",
    correlationId: string | null,
    attributes: Record<string, string | number | boolean> = {},
    outcome?: "success" | "failure" | "degraded",
  ) {
    const sequence = ++captureAuditSequence;
    const captureSessionId = getSessionId();
    const observedAtMs = Date.now();
    attributes = {
      ...attributes,
      sequence,
      observedAtMs,
      captureSessionId: captureSessionId ?? "",
    };
    const serialized = JSON.stringify(attributes);
    if (new TextEncoder().encode(serialized).length > 1_500) {
      const characters = [...serialized];
      const parts = Math.ceil(characters.length / 180);
      for (let index = 0; index < parts; index += 1) {
        recordConversationCaptureAuditEvent({
          eventName: `${eventName}-detail`,
          phase: "progress",
          correlationId,
          attributes: {
            sequence,
            observedAtMs,
            part: index + 1,
            parts,
            text: characters.slice(index * 180, (index + 1) * 180).join(""),
          },
        });
      }
      attributes = {
        sequence,
        observedAtMs,
        overflow: true,
        bytes: new TextEncoder().encode(serialized).length,
      };
    }
    recordConversationCaptureAuditEvent({ eventName, phase, outcome, correlationId, attributes });
  };
}
