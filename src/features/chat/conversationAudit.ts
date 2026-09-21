import { recordAuditEvent } from "../../lib/auditRuntime";
import type { RuntimeEvent } from "../../lib/contracts";
import { isHarnessFailureCode, isHarnessProgressCode } from "../../lib/harnessFailureDiagnostics";

export function recordRuntimeLifecycleAudit(event: RuntimeEvent, conversationId: string) {
  const common = { correlationId: event.runId, conversationId, runtimeRunId: event.runId };
  switch (event.type) {
    case "started":
      recordAuditEvent({
        ...common,
        component: "conversation",
        eventName: "runtime-event-started",
        phase: "start",
        outcome: "success",
        subjectId: event.runId,
        attributes: { routeKind: event.route, providerId: event.providerId },
      });
      break;
    case "activity":
      recordAuditEvent({
        ...common,
        component: "provider",
        eventName: "runtime-activity-received",
        phase: "progress",
        subjectId: event.runId,
        attributes: isHarnessProgressCode(event.kind) ? { reasonCode: event.kind } : {},
      });
      break;
    case "providerFailed":
      recordAuditEvent({
        ...common,
        component: "provider",
        eventName: "runtime-provider-failed",
        phase: "error",
        outcome: "failure",
        subjectId: event.providerId,
        failureCode: isHarnessFailureCode(event.reason) ? event.reason : "provider-failed",
      });
      break;
    case "messageCompleted":
      recordAuditEvent({
        ...common,
        component: "conversation",
        eventName: "runtime-message-completed",
        phase: "terminal",
        outcome: "success",
        subjectId: event.message.id,
      });
      break;
    case "speechStarted":
      recordAuditEvent({
        ...common,
        component: "tts",
        eventName: "speech-started",
        phase: "start",
        outcome: "success",
        subjectId: event.runId,
      });
      break;
    case "speechEnded":
      recordAuditEvent({
        ...common,
        component: "tts",
        eventName: "speech-ended",
        phase: "terminal",
        outcome: "success",
        subjectId: event.runId,
      });
      break;
    case "speechFailed":
      recordAuditEvent({
        ...common,
        component: "tts",
        eventName: "speech-failed",
        phase: "error",
        outcome: "failure",
        subjectId: event.runId,
        failureCode: "speech-failed",
      });
      break;
    case "cancelled":
      recordAuditEvent({
        ...common,
        component: "conversation",
        eventName: "runtime-cancelled",
        phase: "terminal",
        outcome: "cancelled",
        subjectId: event.runId,
        failureCode: "user-cancelled",
      });
      break;
    case "failed":
      recordAuditEvent({
        ...common,
        component: "conversation",
        eventName: "runtime-failed",
        phase: "error",
        outcome: "failure",
        subjectId: event.runId,
        failureCode: event.code,
      });
      break;
    case "delta":
      break;
  }
}
