import { expect, test } from "bun:test";
import { isHarnessFailureCode } from "../src/lib/harnessFailureDiagnostics";

test("only locally defined diagnostic codes can enter audit records", () => {
  expect(isHarnessFailureCode("harness-catalog-schema-invalid")).toBe(true);
  expect(isHarnessFailureCode("harness-remote-secret-value")).toBe(false);
  expect(isHarnessFailureCode("constructor")).toBe(false);
  expect(isHarnessFailureCode("Bearer secret")).toBe(false);
});
