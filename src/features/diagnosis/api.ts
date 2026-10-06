import { invoke } from "@tauri-apps/api/core";
import { z } from "zod";
import type { DiagnosisReport, DiagnosisScope } from "../../lib/generated/diagnosis";

export const capabilityIds = [
  "storage",
  "conversation",
  "voice-listen",
  "voice-speak",
  "voice-echo",
  "memory",
  "coding",
] as const;

const capability = z.enum(capabilityIds);
const state = z.enum(["ready", "degraded", "unavailable", "unverified", "disabled"]);
const reason = z.enum([
  "ok",
  "disabled",
  "not-configured",
  "not-proven",
  "not-observed",
  "expired",
  "timeout",
  "unreachable",
  "auth-failed",
  "not-advertised",
  "not-ready",
  "unavailable",
  "schema-mismatch",
  "capacity-high",
  "recent-failure",
  "capture-failed",
  "aec-inactive",
  "echo-leak",
  "speech-suppressed",
  "internal",
]);

const evidence = z
  .object({
    source: z.string(),
    capability,
    route: z.string(),
    tier: z.enum(["static", "observed", "probe"]),
    importance: z.enum(["required", "advisory"]),
    outcome: z.enum(["pass", "degraded", "fail", "unverified", "disabled"]),
    reason,
    subject: z.string().nullable(),
    detail: z.string().nullable(),
    latencyMs: z.number().nullable(),
    observedAt: z.number(),
    expiresAt: z.number().nullable(),
  })
  .strict();

const reportSchema = z
  .object({
    schemaVersion: z.literal(2),
    revision: z.number(),
    startedAt: z.number(),
    finishedAt: z.number().nullable(),
    running: z.boolean(),
    overall: state,
    capabilities: z.array(
      z
        .object({
          capability,
          state,
          reason,
          verifiedAt: z.number().nullable(),
          optional: z.boolean(),
          actions: z.array(z.enum(["open-settings", "retest"])),
          evidence: z.array(evidence),
        })
        .strict(),
    ),
  })
  .strict();

export function parseDiagnosisReport(value: unknown): DiagnosisReport {
  return reportSchema.parse(value);
}

export async function getDiagnosisReport(): Promise<DiagnosisReport> {
  return parseDiagnosisReport(await invoke("get_diagnosis_report"));
}

export async function runDiagnosis(scope: DiagnosisScope): Promise<DiagnosisReport> {
  return parseDiagnosisReport(await invoke("run_diagnosis", { scope }));
}
