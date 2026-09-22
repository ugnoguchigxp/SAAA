import { invoke } from "@tauri-apps/api/core";
import { z } from "zod";
import type { DiagnosisReport } from "../../lib/generated/diagnosis";

const diagnosisStatus = z.enum(["ok", "warn", "fail", "skipped", "running"]);
const diagnosisSeverity = z.enum(["fatal", "degraded", "info"]);

const diagnosisReportSchema = z
  .object({
    revision: z.number(),
    startedAt: z.string(),
    finishedAt: z.string().nullable(),
    running: z.boolean(),
    overall: diagnosisStatus,
    items: z.array(
      z.object({
        id: z.string(),
        group: z.string(),
        label: z.string(),
        status: diagnosisStatus,
        severity: diagnosisSeverity,
        message: z.string(),
        latencyMs: z.number().nullable(),
      }),
    ),
  })
  .strict();

export function parseDiagnosisReport(value: unknown): DiagnosisReport {
  return diagnosisReportSchema.parse(value);
}

export async function getDiagnosisReport(): Promise<DiagnosisReport> {
  return parseDiagnosisReport(await invoke("get_diagnosis_report"));
}

export async function runDiagnosis(): Promise<DiagnosisReport> {
  return parseDiagnosisReport(await invoke("run_diagnosis"));
}
