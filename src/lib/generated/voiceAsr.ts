// Generated from the Rust voice ASR contract. Do not edit by hand.
export type VoiceAsrFailureCode =
  | "asr-session-exists" | "asr-session-not-found" | "asr-packet-format"
  | "asr-packet-sequence" | "asr-backpressure" | "asr-provider-unavailable"
  | "asr-stream-protocol" | "asr-stream-timeout" | "asr-final-timeout"
  | "asr-target-speaker-unavailable" | "asr-language-not-allowed"
  | "asr-no-speech" | "asr-cancelled";

export type VoiceAsrStreamEvent =
  | { type: "ready"; sessionId: string; currentUtteranceId: string; protocol: "native" | "batch-agreement"; scope: "all-speakers" | "target-speaker" }
  | { type: "partial"; sessionId: string; utteranceId: string; revision: number; startMs: number; endMs: number; stableText: string; unstableText: string; language: string | null }
  | { type: "final"; sessionId: string; utteranceId: string; revision: number; startMs: number; endMs: number; text: string; language: string | null }
  | { type: "utteranceDiscarded"; sessionId: string; utteranceId: string; reason: "no-speech" | "target-speaker-empty" | "cancelled" }
  | { type: "degraded"; sessionId: string; from: "native"; to: "batch-agreement"; reasonCode: VoiceAsrFailureCode }
  | { type: "failed"; sessionId: string; utteranceId: string | null; code: VoiceAsrFailureCode; message: string; recovery: string; fatal: boolean }
  | { type: "stopped"; sessionId: string };

export type StartVoiceAsrSessionInput = { sessionId: string; conversationId: string; sampleRate: number };
export type CommitVoiceAsrUtteranceInput = { sessionId: string; reason: "silence" | "max-duration" };
export type StopVoiceAsrSessionInput = { sessionId: string; finalizeCurrent: boolean };
