import { invoke } from "@tauri-apps/api/core";

import type { AuditEventInput } from "./auditEventInput";
export type { AuditEventInput } from "./auditEventInput";

/** Audit persistence must never delay or fail the user-facing event path. */
export function recordAuditEvent(input: AuditEventInput): void {
  void invoke<void>("record_frontend_audit_event", { input }).catch(() => undefined);
}

/** Conversation capture has bounded structured details that the generic audit allowlist excludes. */
export function recordConversationCaptureAuditEvent(
  input: Pick<AuditEventInput, "eventName" | "phase" | "outcome" | "correlationId" | "attributes">,
): void {
  void invoke<void>("record_conversation_capture_audit_event", { input }).catch((error) => {
    console.error("Conversation ASR audit write failed", error);
  });
}
