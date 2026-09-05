// Generated from src-tauri/src/voice/streaming_asr/contracts.rs. Do not edit by hand.
// Run `bun run ipc:generate` after changing the Rust voice ASR contract.

export type VoiceAsrFailureCode = "asr-session-exists" | "asr-session-not-found" | "asr-packet-format" | "asr-packet-sequence" | "asr-backpressure" | "asr-provider-unavailable" | "asr-stream-protocol" | "asr-stream-timeout" | "asr-final-timeout" | "asr-target-speaker-unavailable" | "asr-language-not-allowed" | "asr-no-speech" | "asr-cancelled";

export type VoiceAsrStreamEvent = { "type": "ready", sessionId: string, currentUtteranceId: string, protocol: "native" | "batch-agreement", scope: "all-speakers" | "target-speaker", } | { "type": "partial", sessionId: string, utteranceId: string, revision: number, startMs: number, endMs: number, stableText: string, unstableText: string, language: string | null, } | { "type": "utteranceDiscarded", sessionId: string, utteranceId: string, reason: "no-speech" | "target-speaker-empty" | "cancelled", } | { "type": "final", sessionId: string, utteranceId: string, revision: number, startMs: number, endMs: number, text: string, language: string | null, } | { "type": "failed", sessionId: string, utteranceId: string | null, code: VoiceAsrFailureCode, message: string, recovery: string, fatal: boolean, } | { "type": "degraded", sessionId: string, from: "native", to: "batch-agreement", reasonCode: VoiceAsrFailureCode, } | { "type": "stopped", sessionId: string, };

export type CommitReason = "silence" | "max-duration";

export type StartVoiceAsrSessionInput = { sessionId: string, conversationId: string, sampleRate: number, recoverExisting?: boolean, };

export type CommitVoiceAsrUtteranceInput = { sessionId: string, reason: CommitReason, };

export type StopVoiceAsrSessionInput = { sessionId: string, finalizeCurrent: boolean, };
