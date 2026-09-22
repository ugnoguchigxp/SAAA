// Generated from src-tauri/src/diagnosis/contract.rs. Do not edit.
export type DiagnosisStatus = "ok" | "warn" | "fail" | "skipped" | "running";
export type DiagnosisSeverity = "fatal" | "degraded" | "info";
export type DiagnosisItem = { id: string, group: string, label: string, status: DiagnosisStatus, severity: DiagnosisSeverity, message: string, latencyMs: number | null, };
export type DiagnosisReport = { revision: number, startedAt: string, finishedAt: string | null, running: boolean, overall: DiagnosisStatus, items: Array<DiagnosisItem>, };
