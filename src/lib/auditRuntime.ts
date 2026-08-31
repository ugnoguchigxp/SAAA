import { invoke } from "@tauri-apps/api/core";

export type AuditEventInput = {
  component: "app" | "frontend" | "microphone" | "voice-asr" | "conversation" | "provider" | "tts" | "meeting" | "settings" | "voice-policy" | "situation";
  eventName: string;
  phase: "request" | "start" | "state" | "progress" | "decision" | "terminal" | "error";
  outcome?: "success" | "failure" | "cancelled" | "interrupted" | "degraded" | "blocked" | null;
  correlationId?: string | null;
  causationId?: string | null;
  conversationId?: string | null;
  runtimeRunId?: string | null;
  sessionId?: string | null;
  subjectId?: string | null;
  failureCode?: string | null;
  attributes?: Record<string, string | number | boolean>;
};

/** Audit persistence must never delay or fail the user-facing event path. */
export function recordAuditEvent(input: AuditEventInput): void {
  void invoke<void>("record_frontend_audit_event", { input }).catch(() => undefined);
}
