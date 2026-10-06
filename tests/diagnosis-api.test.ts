import { expect, test } from "bun:test";
import { invokeCalls, invokeImpl, resetTauriCoreMock } from "./tauriCoreMock";
import { mixed, report } from "./diagnosisFixtures";

const { getDiagnosisReport, parseDiagnosisReport, runDiagnosis } =
  await import("../src/features/diagnosis/api");

test("diagnosis api reads a report and sends the scope as a tagged object", async () => {
  resetTauriCoreMock();
  invokeImpl.handler = async (command) =>
    command === "run_diagnosis" ? { ...mixed, revision: 2 } : mixed;
  expect((await getDiagnosisReport()).revision).toBe(1);
  expect((await runDiagnosis({ kind: "capability", capability: "voice-listen" })).revision).toBe(2);
  const run = invokeCalls.find((call) => call.command === "run_diagnosis");
  expect(run?.args).toEqual({ scope: { kind: "capability", capability: "voice-listen" } });
});

test("diagnosis api rejects unknown states, reasons and schema versions", () => {
  expect(() => parseDiagnosisReport({ ...mixed, overall: "bogus" })).toThrow();
  expect(() => parseDiagnosisReport({ ...mixed, schemaVersion: 1 })).toThrow();
  expect(() => parseDiagnosisReport({ ...mixed, extra: true })).toThrow();
  const broken = structuredClone(mixed);
  (broken.capabilities[0] as { reason: string }).reason = "made-up";
  expect(() => parseDiagnosisReport(broken)).toThrow();
  expect(parseDiagnosisReport(report("ready", [])).capabilities).toEqual([]);
});
