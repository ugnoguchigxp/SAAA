import { expect, test } from "bun:test";
import { invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";

const { getDiagnosisReport, runDiagnosis, runFastDiagnosis } =
  await import("../src/features/diagnosis/api");

const report = {
  revision: 1,
  startedAt: "2026-09-22T00:00:00Z",
  finishedAt: "2026-09-22T00:00:01Z",
  running: false,
  overall: "ok",
  items: [
    {
      id: "sqlite",
      group: "storage",
      label: "SQLite",
      status: "ok",
      severity: "fatal",
      message: "",
      latencyMs: null,
    },
  ],
};

test("diagnosis api accepts a report and rejects an unknown status", async () => {
  resetTauriCoreMock();
  invokeImpl.handler = async (command) => {
    if (command === "run_diagnosis") return { ...report, revision: 2 };
    if (command === "run_fast_diagnosis") return { ...report, revision: 3 };
    return report;
  };
  expect((await getDiagnosisReport()).revision).toBe(1);
  expect((await runDiagnosis()).revision).toBe(2);
  expect((await runFastDiagnosis()).revision).toBe(3);
  invokeImpl.handler = async () => ({ ...report, overall: "bogus" });
  await expect(getDiagnosisReport()).rejects.toThrow();
});
