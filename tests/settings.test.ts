import { describe, expect, test } from "bun:test";
import { validateSettingsDocuments } from "../src/lib/schemas";

function documents() {
  return [
    {
      namespace: "providers.model",
      key: "default",
      schemaVersion: 7,
      valueJson: {
        providers: [{ id: "local", enabled: true, label: "Local", location: "local", endpoint: "http://127.0.0.1:11434/v1", model: "test", credentialStatus: "not-configured" }],
      },
    },
    {
      namespace: "providers.agent",
      key: "codex-sdk",
      schemaVersion: 7,
      valueJson: { enabled: false, provider: "codex-sdk", model: "", runtimeMode: "app-server", health: "unchecked", sandboxMode: "read-only", approvalPolicy: "never", networkEnabled: false, webSearchEnabled: false, workspacePolicy: "select-per-conversation" },
    },
    {
      namespace: "routing.tasks",
      key: "default",
      schemaVersion: 7,
      valueJson: { conversationRespond: { primaryProviderId: "local", fallbackProviderIds: [], timeoutMs: 30_000 }, codingAssist: { providerId: "codex-sdk", timeoutMs: 120_000, readOnly: true, networkEnabled: false, webSearchEnabled: false } },
    },
    {
      namespace: "voice.runtime",
      key: "default",
      schemaVersion: 7,
      valueJson: { inputDeviceId: "default", outputDeviceId: "default", captureMode: "push-to-talk", sttProviderId: "local-whisper", sttModel: "", ttsProviderId: "system-tts", ttsVoice: "default", autoSpeak: true, cloudFallbackEnabled: false },
    },
    {
      namespace: "security.runtime",
      key: "default",
      schemaVersion: 7,
      valueJson: { credentialStorage: "environment", localOnlyWhenSelected: true, diagnosticsRedaction: true },
    },
    {
      namespace: "situation.runtime",
      key: "default",
      schemaVersion: 7,
      valueJson: { enabled: false, sampleIntervalMs: 2_000, calendarEnabled: false, retentionDays: 7, maxLedgerEntries: 10_000, heartbeatIntervalMs: 300_000, sensitiveApplicationCategories: true },
    },
  ];
}

describe("settings contracts", () => {
  test("accepts the complete MVP settings snapshot", () => {
    expect(() => validateSettingsDocuments(documents())).not.toThrow();
  });

  test("keeps schema 6 provider identifiers and default Codex models readable", () => {
    const snapshot = documents();
    const provider = (snapshot[0].valueJson as { providers: Array<{ id: string }> }).providers[0];
    provider.id = "Local_Custom";
    (snapshot[2].valueJson as { conversationRespond: { primaryProviderId: string } }).conversationRespond.primaryProviderId = "Local_Custom";
    const codex = snapshot[1].valueJson as { enabled: boolean; model: string };
    codex.enabled = true;
    codex.model = "";
    expect(() => validateSettingsDocuments(snapshot)).not.toThrow();
  });

  test("rejects credentials embedded in provider endpoints", () => {
    const snapshot = documents();
    (snapshot[0].valueJson as { providers: Array<{ endpoint: string }> }).providers[0].endpoint = "http://user:secret@127.0.0.1:11434/v1";
    expect(() => validateSettingsDocuments(snapshot)).toThrow("Credentials must not be embedded");
  });

  test("rejects unsafe or credential-ambiguous provider ids", () => {
    const unsafe = documents();
    (unsafe[0].valueJson as { providers: Array<{ id: string }> }).providers[0].id = "local provider";
    expect(() => validateSettingsDocuments(unsafe)).toThrow("Provider ids may contain only");

    const ambiguous = documents();
    const providers = (ambiguous[0].valueJson as { providers: Array<Record<string, unknown>> }).providers;
    providers[0].id = "local-a";
    providers.push({ id: "local_a", enabled: false, label: "Local duplicate", location: "local", endpoint: "", model: "", credentialStatus: "not-configured" });
    (ambiguous[2].valueJson as { conversationRespond: { primaryProviderId: string } }).conversationRespond.primaryProviderId = "local-a";
    expect(() => validateSettingsDocuments(ambiguous)).toThrow("credential environment variable");
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

  test("rejects unknown fields instead of silently dropping them", () => {
    const snapshot = documents();
    (snapshot[4].valueJson as Record<string, unknown>).unexpectedPolicy = true;
    expect(() => validateSettingsDocuments(snapshot)).toThrow("Unrecognized key");
  });

  test("accepts schema 7 and rejects schema 6", () => {
    expect(() => validateSettingsDocuments(documents())).not.toThrow();
    const legacy = documents();
    legacy[0].schemaVersion = 6;
    expect(() => validateSettingsDocuments(legacy)).toThrow("Invalid input");
  });
});
