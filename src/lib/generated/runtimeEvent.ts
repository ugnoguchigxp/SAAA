// Generated from src-tauri/src/ipc_contract.rs. Do not edit by hand.
// Run `bun run ipc:generate` after changing the Rust IPC contract.

import type { ContentPart } from "./generativeUi";

export type ConversationMessage = { id: string, conversationId: string, role: "user" | "assistant" | "system" | "transcript", content: string, parts?: Array<ContentPart>, createdAt: string, };

export const runtimeFailureCodes = ["runtime_error", "configuration-error", "child-start-failed", "request-timeout", "progress-timeout", "terminal-timeout", "hard-timeout", "child-exited", "protocol-error", "policy-violation", "provider-error", "response-too-large", "required-context-overflow", "context-scope-changed", "required-context-unavailable", "internal-error"] as const;
export type RuntimeFailureCode = (typeof runtimeFailureCodes)[number];

export type VoicePresentationDecision = { decision: "speak" | "silent", reasonCode: "global_opt_out" | "turn_override" | "conversation_override" | "global_default" | "route_blocked" | "situation_hold", };

export type ConversationVoicePolicySnapshot = { conversationId: string, speechOutput: "inherit" | "muted", listeningPace: "inherit" | "quick" | "balanced" | "patient", policyRevision: number, updatedAt: string, effectiveSpeechOutput: "speak" | "silent", speechReasonCode: "global_opt_out" | "conversation_override" | "global_default" | "situation_hold", effectiveListeningPace: "quick" | "balanced" | "patient", effectiveSilenceTimeoutMs: number, };

export type RuntimeEvent = { "type": "started", runId: string, route: string, providerId: string, } | { "type": "delta", runId: string, text: string, } | { "type": "activity", runId: string, kind: string, summary: string, } | { "type": "providerFailed", runId: string, providerId: string, reason: string, } | { "type": "messageCompleted", runId: string, message: ConversationMessage, presentation: VoicePresentationDecision, voicePolicy: ConversationVoicePolicySnapshot | null, } | { "type": "speechStarted", runId: string, } | { "type": "speechEnded", runId: string, } | { "type": "speechFailed", runId: string, message: string, recovery: string, } | { "type": "cancelled", runId: string, } | { "type": "failed", runId: string, code: RuntimeFailureCode, message: string, recovery: string, };

// Role-routing reconnect projections.

export type RoutingSnapshotInput = { conversationId: string, };

export type RoutingEventReplayInput = { rootId: string, afterSeq: bigint, };

export type RoutingCancelInput = { rootId: string, };

export type AdaptiveRollbackInput = { artifactId: string, };

export type AdaptiveArtifactSnapshot = { id: string, domain: string, scopeKey: string, state: string, eligibleExamples: bigint, bestObservedScore: number | null, policyRevision: bigint | null, reason: string, };

export type RoutingLearningSnapshot = { dirtyRoots: bigint, readyDatasets: bigint, activeArtifacts: bigint, adaptiveArtifacts: Array<AdaptiveArtifactSnapshot>, };

export type RoutingRootSnapshot = { rootId: string, runtimeRunId: string | null, phase: string, revision: number, activeSlot: string | null, cancelRequested: boolean, lastEventSeq: bigint, };

export type RoutingSnapshot = { active: RoutingRootSnapshot | null, queued: Array<RoutingRootSnapshot>, };

export type RoutingEventRecord = { rootId: string, seq: bigint, kind: string, dataJson: string, createdAtMs: bigint, };
