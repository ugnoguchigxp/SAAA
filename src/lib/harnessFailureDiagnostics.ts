// Locally defined codes only. Never persist arbitrary provider error text.
export const harnessFailureHintKeys: Record<string, string> = {
  "harness-catalog-schema-invalid": "audit.monitor.harness.catalogSchema",
  "harness-catalog-version-unsupported": "audit.monitor.harness.catalogVersion",
  "harness-llm-context-window-missing": "audit.monitor.harness.contextWindowMissing",
  "harness-llm-context-window-invalid": "audit.monitor.harness.contextWindowInvalid",
  "harness-claim-schema-invalid": "audit.monitor.harness.claimSchema",
  "harness-health-schema-invalid": "audit.monitor.harness.healthSchema",
  "harness-connection-schema-invalid": "audit.monitor.harness.connectionSchema",
};

export function isHarnessFailureCode(value: string): boolean {
  return Object.prototype.hasOwnProperty.call(harnessFailureHintKeys, value);
}

export const harnessProgressHintKeys: Record<string, string> = {
  "harness-connection-preparing": "audit.monitor.harness.preparing",
  "harness-connection-ready": "audit.monitor.harness.ready",
  "llm-request-sending": "audit.monitor.harness.awaitingResponse",
};
export function isHarnessProgressCode(value: string): boolean {
  return Object.prototype.hasOwnProperty.call(harnessProgressHintKeys, value);
}
