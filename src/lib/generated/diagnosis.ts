// Generated from src-tauri/src/diagnosis/contract.rs. Do not edit.
export type DiagnosisCapability = "storage" | "conversation" | "voice-listen" | "voice-speak" | "voice-echo" | "memory" | "coding";
export type DiagnosisTier = "static" | "observed" | "probe";
export type DiagnosisOutcome = "pass" | "degraded" | "fail" | "unverified" | "disabled";
export type DiagnosisImportance = "required" | "advisory";
export type DiagnosisReason = "ok" | "disabled" | "not-configured" | "not-proven" | "not-observed" | "expired" | "timeout" | "unreachable" | "auth-failed" | "not-advertised" | "not-ready" | "unavailable" | "schema-mismatch" | "capacity-high" | "recent-failure" | "capture-failed" | "aec-inactive" | "echo-leak" | "speech-suppressed" | "internal";
export type DiagnosisEvidence = { 
/**
 * Stable machine id, such as `larm.asr` or `runs.conversation`.
 */
source: string, capability: DiagnosisCapability, 
/**
 * `core` evidence must all hold. Other routes are alternatives; one working route is enough.
 */
route: string, tier: DiagnosisTier, importance: DiagnosisImportance, outcome: DiagnosisOutcome, reason: DiagnosisReason, 
/**
 * User-configured name, such as a provider label.
 */
subject: string | null, 
/**
 * Short redacted note. Never a URL or token.
 */
detail: string | null, latencyMs: number | null, observedAt: number, expiresAt: number | null, };
export type DiagnosisState = "ready" | "degraded" | "unavailable" | "unverified" | "disabled";
export type DiagnosisAction = "open-settings" | "retest";
export type DiagnosisCapabilityReport = { capability: DiagnosisCapability, state: DiagnosisState, reason: DiagnosisReason, 
/**
 * Newest time a probe or real use proved this capability.
 */
verifiedAt: number | null, optional: boolean, actions: Array<DiagnosisAction>, evidence: Array<DiagnosisEvidence>, };
export type DiagnosisReport = { schemaVersion: number, revision: number, startedAt: number, finishedAt: number | null, running: boolean, overall: DiagnosisState, capabilities: Array<DiagnosisCapabilityReport>, };
export type DiagnosisScope = { "kind": "quick" } | { "kind": "full" } | { "kind": "capability", capability: DiagnosisCapability, };
