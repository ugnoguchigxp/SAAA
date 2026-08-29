import { describe, expect, test } from "bun:test";
import { validateSettingsDocuments } from "../src/lib/schemas";

function documents() {
  return [
    {
      namespace: "providers.model",
      key: "default",
      schemaVersion: 9,
      valueJson: {
        providers: [{ kind: "openai-compatible", id: "local", enabled: true, label: "Local", location: "local", endpoint: "http://127.0.0.1:11434/v1", model: "test", credentialStatus: "not-configured" }],
        reasoningEffort: "medium",
      },
    },
    {
      namespace: "providers.agent",
      key: "codex-sdk",
      schemaVersion: 9,
      valueJson: { enabled: false, provider: "codex-sdk", model: "", runtimeMode: "app-server", health: "unchecked", sandboxMode: "read-only", approvalPolicy: "never", networkEnabled: false, webSearchEnabled: false, workspacePolicy: "select-per-conversation" },
    },
    {
      namespace: "routing.tasks",
      key: "default",
      schemaVersion: 9,
      valueJson: { conversationRespond: { primaryProviderId: "local", fallbackProviderIds: [], timeoutMs: 30_000 }, codingAssist: { providerId: "codex-sdk", timeoutMs: 120_000, readOnly: true, networkEnabled: false, webSearchEnabled: false } },
    },
    {
      namespace: "voice.runtime",
      key: "default",
      schemaVersion: 9,
      valueJson: { inputDeviceId: "default", outputDeviceId: "default", captureMode: "push-to-talk", sttProviderId: "gnosis-asr", sttModel: "qwen3-asr-1.7b", ttsProviderId: "system-tts", ttsVoice: "default", autoSpeak: true, cloudFallbackEnabled: false },
    },
    {
      namespace: "security.runtime",
      key: "default",
      schemaVersion: 9,
      valueJson: { credentialStorage: "environment", localOnlyWhenSelected: true, diagnosticsRedaction: true },
    },
    {
      namespace: "situation.runtime",
      key: "default",
      schemaVersion: 9,
      valueJson: { enabled: false, sampleIntervalMs: 2_000, calendarEnabled: false, retentionDays: 7, maxLedgerEntries: 10_000, heartbeatIntervalMs: 300_000, sensitiveApplicationCategories: true },
    },
  ];
}

describe("settings contracts", () => {
  test("accepts the complete MVP settings snapshot", () => {
    expect(() => validateSettingsDocuments(documents())).not.toThrow();
  });

  test("accepts only supported conversation reasoning efforts", () => {
    for (const reasoningEffort of ["low", "medium", "xhigh"]) {
      const snapshot = documents();
      (snapshot[0].valueJson as { reasoningEffort: string }).reasoningEffort = reasoningEffort;
      expect(() => validateSettingsDocuments(snapshot)).not.toThrow();
    }
    const invalid = documents();
    (invalid[0].valueJson as { reasoningEffort: string }).reasoningEffort = "mid";
    expect(() => validateSettingsDocuments(invalid)).toThrow("Invalid option");
  });

  test("requires the fixed gnosis ASR provider and model", () => {
    const localWhisper = documents();
    (localWhisper[3].valueJson as { sttProviderId: string }).sttProviderId = "local-whisper";
    expect(() => validateSettingsDocuments(localWhisper)).toThrow("Invalid input");

    const wrongModel = documents();
    (wrongModel[3].valueJson as { sttModel: string }).sttModel = "ggml-base.bin";
    expect(() => validateSettingsDocuments(wrongModel)).toThrow("Invalid input");
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

  test("accepts private-network local providers and rejects public HTTP endpoints", () => {
    const privateNetwork = documents();
    (privateNetwork[0].valueJson as { providers: Array<{ endpoint: string }> }).providers[0].endpoint = "http://192.168.0.65:8080/v1";
    expect(() => validateSettingsDocuments(privateNetwork)).not.toThrow();

    const publicNetwork = documents();
    (publicNetwork[0].valueJson as { providers: Array<{ endpoint: string }> }).providers[0].endpoint = "http://203.0.113.10:8080/v1";
    expect(() => validateSettingsDocuments(publicNetwork)).toThrow("loopback or private-network");
  });

  test("accepts a host-only gnosis provider and rejects embedded URLs or ports", () => {
    const snapshot = documents();
    (snapshot[0].valueJson as { providers: unknown[] }).providers = [{
      kind: "gnosis",
      id: "gnosis-qwen",
      enabled: true,
      label: "gnosis · Dynamic LLM",
      location: "local",
      host: "192.168.0.65",
    }];
    (snapshot[2].valueJson as { conversationRespond: { primaryProviderId: string } }).conversationRespond.primaryProviderId = "gnosis-qwen";
    expect(() => validateSettingsDocuments(snapshot)).not.toThrow();
    const localName = structuredClone(snapshot);
    ((localName[0].valueJson as { providers: Array<{ host: string }> }).providers[0]).host = "Gnosis.LOCAL";
    expect(() => validateSettingsDocuments(localName)).not.toThrow();

    for (const host of [
      "http://192.168.0.65",
      "192.168.0.65:9810",
      "example.com",
      "gnosis-",
      "foo..local",
      "foo-.local",
    ]) {
      const invalid = structuredClone(snapshot);
      ((invalid[0].valueJson as { providers: Array<{ host: string }> }).providers[0]).host = host;
      expect(() => validateSettingsDocuments(invalid)).toThrow();
    }
  });

  test("rejects every fallback behind a gnosis primary", () => {
    const snapshot = documents();
    const providers = (snapshot[0].valueJson as { providers: Array<Record<string, unknown>> }).providers;
    providers[0] = { kind: "gnosis", id: "gnosis-qwen", enabled: true, label: "gnosis", location: "local", host: "192.168.0.65" };
    providers.push({ kind: "openai-compatible", id: "local-fallback", enabled: true, label: "Fallback", location: "local", endpoint: "http://127.0.0.1:11435/v1", model: "test", credentialStatus: "not-configured" });
    const route = (snapshot[2].valueJson as { conversationRespond: { primaryProviderId: string; fallbackProviderIds: string[] } }).conversationRespond;
    route.primaryProviderId = "gnosis-qwen";
    route.fallbackProviderIds = ["local-fallback"];
    expect(() => validateSettingsDocuments(snapshot)).toThrow("must not configure fallback");
  });

  test("rejects a gnosis timeout that cannot fit within its connection lifetime", () => {
    const snapshot = documents();
    const providers = (snapshot[0].valueJson as { providers: Array<Record<string, unknown>> }).providers;
    providers[0] = { kind: "gnosis", id: "gnosis-qwen", enabled: true, label: "gnosis", location: "local", host: "192.168.0.65" };
    const route = (snapshot[2].valueJson as { conversationRespond: { primaryProviderId: string; timeoutMs: number } }).conversationRespond;
    route.primaryProviderId = "gnosis-qwen";
    route.timeoutMs = 270_000;
    expect(() => validateSettingsDocuments(snapshot)).toThrow("gnosis conversation timeout must not exceed 269999 ms");
    route.timeoutMs = 269_999;
    expect(() => validateSettingsDocuments(snapshot)).not.toThrow();
  });

  test("rejects unsafe or credential-ambiguous provider ids", () => {
    const unsafe = documents();
    (unsafe[0].valueJson as { providers: Array<{ id: string }> }).providers[0].id = "local provider";
    expect(() => validateSettingsDocuments(unsafe)).toThrow("Provider ids may contain only");

    const ambiguous = documents();
    const providers = (ambiguous[0].valueJson as { providers: Array<Record<string, unknown>> }).providers;
    providers[0].id = "local-a";
    providers.push({ kind: "openai-compatible", id: "local_a", enabled: false, label: "Local duplicate", location: "local", endpoint: "", model: "", credentialStatus: "not-configured" });
    (ambiguous[2].valueJson as { conversationRespond: { primaryProviderId: string } }).conversationRespond.primaryProviderId = "local-a";
    expect(() => validateSettingsDocuments(ambiguous)).toThrow("credential environment variable");
  });

  test("rejects cloud fallback behind a local-only primary", () => {
    const snapshot = documents();
    const providers = (snapshot[0].valueJson as { providers: Array<Record<string, unknown>> }).providers;
    providers.push({ kind: "openai-compatible", id: "cloud", enabled: true, label: "Cloud", location: "cloud", endpoint: "https://example.com/v1", model: "test", credentialStatus: "configured" });
    (snapshot[2].valueJson as { conversationRespond: { fallbackProviderIds: string[] } }).conversationRespond.fallbackProviderIds = ["cloud"];
    expect(() => validateSettingsDocuments(snapshot)).toThrow("Cloud fallback is blocked");
  });

  test("rejects duplicate fallback providers", () => {
    const snapshot = documents();
    const providers = (snapshot[0].valueJson as { providers: Array<Record<string, unknown>> }).providers;
    providers.push({ kind: "openai-compatible", id: "fallback", enabled: true, label: "Fallback", location: "local", endpoint: "http://127.0.0.1:11435/v1", model: "local", credentialStatus: "not-configured" });
    (snapshot[2].valueJson as { conversationRespond: { fallbackProviderIds: string[] } }).conversationRespond.fallbackProviderIds = ["fallback", "fallback"];
    expect(() => validateSettingsDocuments(snapshot)).toThrow("Duplicate provider in route: fallback");
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

  test("accepts schema 9 and rejects schema 8", () => {
    expect(() => validateSettingsDocuments(documents())).not.toThrow();
    const legacy = documents();
    legacy[0].schemaVersion = 8;
    expect(() => validateSettingsDocuments(legacy)).toThrow("Invalid input");
  });

  test("accepts only the fixed LARM security contract", () => {
    const snapshot = documents();
    const providers = (snapshot[0].valueJson as { providers: Array<Record<string, unknown>> }).providers;
    providers.push({
      kind: "larm",
      id: "larm-local",
      enabled: false,
      label: "LARM",
      location: "local",
      baseUrl: "http://127.0.0.1:9810",
      tokenEnv: "LARM_API_TOKEN",
      allocationTtlSeconds: 300,
      allocationStartupTimeoutSeconds: 300,
      allowFallbackByDefault: false,
      deploymentPolicy: "existing-only",
    });
    expect(() => validateSettingsDocuments(snapshot)).not.toThrow();
    providers[1].baseUrl = "http://[::1]:9810";
    expect(() => validateSettingsDocuments(snapshot)).not.toThrow();

    for (const baseUrl of [
      "http://localhost:9810",
      "http://192.168.1.10:9810",
      "https://127.0.0.1:9810",
      "http://127.0.0.1:9810/v1",
      "http://user:secret@127.0.0.1:9810",
    ]) {
      providers[1].baseUrl = baseUrl;
      expect(() => validateSettingsDocuments(snapshot)).toThrow("numeric-loopback");
    }
  });

  test("rejects multiple enabled LARM providers", () => {
    const snapshot = documents();
    const providers = (snapshot[0].valueJson as { providers: Array<Record<string, unknown>> }).providers;
    const larm = {
      kind: "larm",
      enabled: true,
      label: "LARM",
      location: "local",
      baseUrl: "http://127.0.0.1:9810",
      tokenEnv: "LARM_API_TOKEN",
      allocationTtlSeconds: 300,
      allocationStartupTimeoutSeconds: 300,
      allowFallbackByDefault: false,
      deploymentPolicy: "existing-only",
    };
    providers.push({ ...larm, id: "larm-one" }, { ...larm, id: "larm-two", baseUrl: "http://127.0.0.1:9811" });
    expect(() => validateSettingsDocuments(snapshot)).toThrow("Only one LARM provider");
  });
});
