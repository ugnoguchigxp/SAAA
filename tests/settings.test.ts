import { describe, expect, test } from "bun:test";
import { validateSettingsDocuments } from "../src/lib/schemas";

function documents() {
  return [
    {
      namespace: "providers.model",
      key: "default",
      schemaVersion: 6,
      valueJson: {
        providers: [{ id: "local", enabled: true, label: "Local", location: "local", endpoint: "http://127.0.0.1:11434/v1", model: "test", credentialStatus: "not-configured" }],
      },
    },
    {
      namespace: "providers.agent",
      key: "codex-sdk",
      schemaVersion: 6,
      valueJson: { enabled: false, provider: "codex-sdk", model: "", runtimeMode: "app-server", health: "unchecked", sandboxMode: "read-only", approvalPolicy: "never", networkEnabled: false, webSearchEnabled: false, workspacePolicy: "select-per-conversation" },
    },
    {
      namespace: "routing.tasks",
      key: "default",
      schemaVersion: 6,
      valueJson: { conversationRespond: { primaryProviderId: "local", fallbackProviderIds: [], timeoutMs: 30_000 }, codingAssist: { providerId: "codex-sdk", timeoutMs: 120_000, readOnly: true, networkEnabled: false, webSearchEnabled: false } },
    },
    {
      namespace: "voice.runtime",
      key: "default",
      schemaVersion: 6,
      valueJson: { inputDeviceId: "default", outputDeviceId: "default", captureMode: "push-to-talk", sttProviderId: "local-whisper", sttModel: "", ttsProviderId: "system-tts", ttsVoice: "default", autoSpeak: true, cloudFallbackEnabled: false },
    },
    {
      namespace: "security.runtime",
      key: "default",
      schemaVersion: 6,
      valueJson: { credentialStorage: "environment", localOnlyWhenSelected: true, diagnosticsRedaction: true },
    },
    {
      namespace: "situation.runtime",
      key: "default",
      schemaVersion: 6,
      valueJson: { enabled: false, sampleIntervalMs: 2_000, calendarEnabled: false, retentionDays: 7, maxLedgerEntries: 10_000, heartbeatIntervalMs: 300_000, sensitiveApplicationCategories: true },
    },
  ];
}

describe("settings contracts", () => {
  test("accepts the complete MVP settings snapshot", () => {
    expect(() => validateSettingsDocuments(documents())).not.toThrow();
  });

  test("rejects credentials embedded in provider endpoints", () => {
    const snapshot = documents();
    (snapshot[0].valueJson as { providers: Array<{ endpoint: string }> }).providers[0].endpoint = "http://user:secret@127.0.0.1:11434/v1";
    expect(() => validateSettingsDocuments(snapshot)).toThrow("Credentials must not be embedded");
  });

  test("rejects cloud fallback behind a local-only primary", () => {
    const snapshot = documents();
    const providers = (snapshot[0].valueJson as { providers: Array<Record<string, unknown>> }).providers;
    providers.push({ id: "cloud", enabled: true, label: "Cloud", location: "cloud", endpoint: "https://example.com/v1", model: "test", credentialStatus: "configured" });
    (snapshot[2].valueJson as { conversationRespond: { fallbackProviderIds: string[] } }).conversationRespond.fallbackProviderIds = ["cloud"];
    expect(() => validateSettingsDocuments(snapshot)).toThrow("Cloud fallback is blocked");
  });

  test("rejects valid namespaces paired with the wrong settings keys", () => {
    const snapshot = documents();
    snapshot[0].key = "codex-sdk";
    snapshot[1].key = "default";
    expect(() => validateSettingsDocuments(snapshot)).toThrow("Each supported settings document");
  });
});
