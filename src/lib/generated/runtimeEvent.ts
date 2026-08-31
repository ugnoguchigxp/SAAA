// Generated from src-tauri/src/ipc_contract.rs. Do not edit by hand.
// Run `bun run ipc:generate` after changing the Rust IPC contract.

export type ConversationMessage = { id: string, conversationId: string, role: "user" | "assistant" | "system" | "transcript", content: string, createdAt: string, };

export const runtimeFailureCodes = ["runtime_error", "configuration-error", "child-start-failed", "request-timeout", "progress-timeout", "terminal-timeout", "hard-timeout", "child-exited", "protocol-error", "policy-violation", "provider-error", "response-too-large", "internal-error"] as const;
export type RuntimeFailureCode = (typeof runtimeFailureCodes)[number];

export type VoicePresentationDecision = { decision: "speak" | "silent", reasonCode: "meeting_blocked" | "global_opt_out" | "turn_override" | "conversation_override" | "global_default" | "route_blocked", };

export type ConversationVoicePolicySnapshot = { conversationId: string, speechOutput: "inherit" | "muted", listeningPace: "inherit" | "quick" | "balanced" | "patient", policyRevision: number, updatedAt: string, effectiveSpeechOutput: "speak" | "silent", speechReasonCode: "meeting_blocked" | "global_opt_out" | "conversation_override" | "global_default", effectiveListeningPace: "quick" | "balanced" | "patient", effectiveSilenceTimeoutMs: number, };

export type RuntimeEvent = { "type": "started", runId: string, route: string, providerId: string, } | { "type": "providerSelected", runId: string, providerId: string, providerKind: "larm", routeId: "llm-default", runtimeId: string, fallbackUsed: boolean, selectionReasonCode: "primary" | "other", } | { "type": "delta", runId: string, text: string, } | { "type": "activity", runId: string, kind: string, summary: string, } | { "type": "providerFailed", runId: string, providerId: string, reason: string, } | { "type": "messageCompleted", runId: string, message: ConversationMessage, presentation: VoicePresentationDecision, voicePolicy: ConversationVoicePolicySnapshot | null, } | { "type": "speechStarted", runId: string, } | { "type": "speechEnded", runId: string, } | { "type": "speechFailed", runId: string, message: string, recovery: string, } | { "type": "cancelled", runId: string, } | { "type": "failed", runId: string, code: RuntimeFailureCode, message: string, recovery: string, };
