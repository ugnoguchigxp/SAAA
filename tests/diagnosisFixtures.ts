import type {
  DiagnosisCapability,
  DiagnosisCapabilityReport,
  DiagnosisEvidence,
  DiagnosisReport,
  DiagnosisState,
} from "../src/lib/generated/diagnosis";

export const NOW = Date.now();

export function evidence(overrides: Partial<DiagnosisEvidence> = {}): DiagnosisEvidence {
  return {
    source: "sqlite.schema",
    capability: "storage",
    route: "core",
    tier: "static",
    importance: "required",
    outcome: "pass",
    reason: "ok",
    subject: null,
    detail: null,
    latencyMs: null,
    observedAt: NOW - 60_000,
    expiresAt: null,
    ...overrides,
  };
}

export function capability(
  id: DiagnosisCapability,
  state: DiagnosisState,
  overrides: Partial<DiagnosisCapabilityReport> = {},
): DiagnosisCapabilityReport {
  return {
    capability: id,
    state,
    reason: state === "ready" ? "ok" : "not-proven",
    verifiedAt: state === "ready" ? NOW - 5 * 60_000 : null,
    optional: id === "coding",
    actions: ["retest"],
    evidence: [],
    ...overrides,
  };
}

export function report(
  overall: DiagnosisState,
  capabilities: DiagnosisCapabilityReport[],
  overrides: Partial<DiagnosisReport> = {},
): DiagnosisReport {
  return {
    schemaVersion: 2,
    revision: 1,
    startedAt: NOW - 2_000,
    finishedAt: NOW - 1_000,
    running: false,
    overall,
    capabilities,
    ...overrides,
  };
}

export const mixed = report("unverified", [
  capability("storage", "ready"),
  capability("conversation", "unverified", {
    reason: "not-proven",
    evidence: [
      evidence({
        source: "provider.deepseek",
        capability: "conversation",
        route: "provider:deepseek",
        subject: "DeepSeek V4.1 Flash",
      }),
    ],
  }),
  capability("voice-listen", "unavailable", {
    reason: "unreachable",
    actions: ["open-settings", "retest"],
    evidence: [
      evidence({
        source: "larm.asr",
        capability: "voice-listen",
        route: "larm",
        tier: "probe",
        outcome: "fail",
        reason: "unreachable",
        detail: "connection refused",
        latencyMs: 12,
      }),
    ],
  }),
  capability("voice-speak", "ready"),
  capability("voice-echo", "ready"),
  capability("memory", "degraded", { reason: "not-ready" }),
  capability("coding", "unverified", { reason: "not-observed" }),
]);
