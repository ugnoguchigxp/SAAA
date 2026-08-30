import { describe, expect, test } from "bun:test";
import { validateSettingsDocuments } from "../src/lib/schemas";
function documents() { return [
    {
      namespace: "providers.model",
      key: "default",
      schemaVersion: 12,
      valueJson: {
        harness: { address: "http://localhost:9810" },
        providers: [{ kind: "openai-compatible", id: "local", enabled: true, label: "Local", location: "local", endpoint: "http://127.0.0.1:11434/v1", model: "test", authentication: "none" }],
        reasoningEffort: "medium",
      },
    },
    {
      namespace: "providers.agent",
      key: "codex-sdk",
      schemaVersion: 12,
      valueJson: { agentName: "SAAA", userName: "", enabled: false, provider: "codex-sdk", model: "", runtimeMode: "app-server", health: "unchecked", sandboxMode: "read-only", approvalPolicy: "never", networkEnabled: false, webSearchEnabled: false, workspacePolicy: "select-per-conversation" },
    },
    {
      namespace: "routing.tasks",
      key: "default",
      schemaVersion: 12,
      valueJson: { conversationRespond: { source: "provider", primaryProviderId: "local", fallbackProviderIds: [], timeoutMs: 30_000 }, voiceTranscribe: { source: "harness", providerId: null, timeoutMs: 120_000 }, voiceSpeak: { source: "harness", providerId: null, timeoutMs: 30_000 }, codingAssist: { providerId: "codex-sdk", timeoutMs: 120_000, readOnly: true, networkEnabled: false, webSearchEnabled: false } },
    },
    {
      namespace: "voice.runtime",
      key: "default",
      schemaVersion: 12,
      valueJson: { listeningEnabled: true, inputDeviceId: "default", outputDeviceId: "default", vadSensitivity: "medium", silenceTimeoutMs: 1500, allowedLanguages: ["ja"], autoSpeak: true },
    },
    {
      namespace: "security.runtime",
      key: "default",
      schemaVersion: 12,
      valueJson: { localOnlyWhenSelected: true, diagnosticsRedaction: true },
    },
    {
      namespace: "situation.runtime",
      key: "default",
      schemaVersion: 12,
      valueJson: { enabled: false, sampleIntervalMs: 2_000, calendarEnabled: false, retentionDays: 7, maxLedgerEntries: 10_000, heartbeatIntervalMs: 300_000, sensitiveApplicationCategories: true },
    },
    { namespace: "ui.preferences", key: "default", schemaVersion: 12, valueJson: { language: "system", timeZone: "system", lengthUnit: "metric", weightUnit: "kilogram", currency: "JPY" } },
  ];
}
describe("settings contracts", () => {
  test("accepts the complete MVP settings snapshot", () => {
    expect(() => validateSettingsDocuments(documents())).not.toThrow();
  });
  test("accepts a bounded agent name and rejects empty or control-character names", () => {
    const localized = documents();
    (localized[1].valueJson as { agentName: string }).agentName = "こはく";
    expect(() => validateSettingsDocuments(localized)).not.toThrow();
    for (const agentName of ["", "line\nbreak", "a".repeat(81)]) {
      const invalid = documents();
      (invalid[1].valueJson as { agentName: string }).agentName = agentName;
      expect(() => validateSettingsDocuments(invalid)).toThrow();
    }
  });
  test("accepts an empty or bounded user name and rejects control characters", () => {
    for (const userName of ["", "野口"]) {
      const snapshot = documents();
      (snapshot[1].valueJson as { userName: string }).userName = userName;
      expect(() => validateSettingsDocuments(snapshot)).not.toThrow();
    }
    const invalid = documents();
    (invalid[1].valueJson as { userName: string }).userName = "太郎\nさん";
    expect(() => validateSettingsDocuments(invalid)).toThrow();
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
  test("validates the provider harness address", () => {
    for (const address of ["http://localhost:9810", "http://[fd00::1]:9810", "https://harness.example.com"]) {
      const snapshot = documents();
      (snapshot[0].valueJson as { harness: { address: string } }).harness.address = address;
      expect(() => validateSettingsDocuments(snapshot)).not.toThrow();
    }
    for (const address of ["http://example.com", "http://[2001:db8::1]", "https://user:secret@example.com"]) {
      const invalid = documents();
      (invalid[0].valueJson as { harness: { address: string } }).harness.address = address;
      expect(() => validateSettingsDocuments(invalid)).toThrow();
    }
  });
  test("bounds continuous listening endpoint settings", () => {
    const valid = documents();
    (valid[3].valueJson as { vadSensitivity: string; silenceTimeoutMs: number }).vadSensitivity = "high";
    expect(() => validateSettingsDocuments(valid)).not.toThrow();
    const invalid = documents();
    (invalid[3].valueJson as { silenceTimeoutMs: number }).silenceTimeoutMs = 500;
    expect(() => validateSettingsDocuments(invalid)).toThrow("Too small");
  });
  test("requires at least one unique supported ASR language", () => {
    const multilingual = documents();
    (multilingual[3].valueJson as { allowedLanguages: string[] }).allowedLanguages = ["ja", "en"];
    expect(() => validateSettingsDocuments(multilingual)).not.toThrow();
    for (const allowedLanguages of [[], ["xx"], ["ja", "ja"]]) {
      const invalid = documents();
      (invalid[3].valueJson as { allowedLanguages: string[] }).allowedLanguages = allowedLanguages;
      expect(() => validateSettingsDocuments(invalid)).toThrow();
    }
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
    expect(() => validateSettingsDocuments(snapshot)).toThrow("must not contain credentials");
  });
  test("accepts private-network local providers and rejects public HTTP endpoints", () => {
    const privateNetwork = documents();
    (privateNetwork[0].valueJson as { providers: Array<{ endpoint: string }> }).providers[0].endpoint = "http://10.0.0.42:8080/v1";
    expect(() => validateSettingsDocuments(privateNetwork)).not.toThrow();
    const publicNetwork = documents();
    (publicNetwork[0].valueJson as { providers: Array<{ endpoint: string }> }).providers[0].endpoint = "http://203.0.113.10:8080/v1";
    expect(() => validateSettingsDocuments(publicNetwork)).toThrow("loopback or private-network");
  });
  test("accepts a host-only dynamic LAN provider and rejects embedded URLs or ports", () => {
    const snapshot = documents();
    (snapshot[0].valueJson as { providers: unknown[] }).providers = [{
      kind: "dynamic-lan",
      id: "lan-llm-dynamic",
      enabled: true,
      label: "LAN LLM · Dynamic connection",
      location: "local",
      host: "10.0.0.42",
    }];
    (snapshot[2].valueJson as { conversationRespond: { primaryProviderId: string } }).conversationRespond.primaryProviderId = "lan-llm-dynamic";
    expect(() => validateSettingsDocuments(snapshot)).not.toThrow();
    const localName = structuredClone(snapshot);
    ((localName[0].valueJson as { providers: Array<{ host: string }> }).providers[0]).host = "DynamicLan.LOCAL";
    expect(() => validateSettingsDocuments(localName)).not.toThrow();
    for (const host of [
      "http://10.0.0.42",
      "10.0.0.42:9810",
      "example.com",
      "dynamic_lan-",
      "foo..local",
      "foo-.local",
    ]) {
      const invalid = structuredClone(snapshot);
      ((invalid[0].valueJson as { providers: Array<{ host: string }> }).providers[0]).host = host;
      expect(() => validateSettingsDocuments(invalid)).toThrow();
    }
  });
  test("accepts a local fallback behind a dynamic_lan primary", () => {
    const snapshot = documents();
    const providers = (snapshot[0].valueJson as { providers: Array<Record<string, unknown>> }).providers;
    providers[0] = { kind: "dynamic-lan", id: "lan-llm-dynamic", enabled: true, label: "dynamic-lan", location: "local", host: "10.0.0.42" };
    providers.push({ kind: "openai-compatible", id: "local-fallback", enabled: true, label: "Fallback", location: "local", endpoint: "http://127.0.0.1:11435/v1", model: "test", authentication: "none" });
    const route = (snapshot[2].valueJson as { conversationRespond: { primaryProviderId: string; fallbackProviderIds: string[] } }).conversationRespond;
    route.primaryProviderId = "lan-llm-dynamic";
    route.fallbackProviderIds = ["local-fallback"];
    expect(() => validateSettingsDocuments(snapshot)).not.toThrow();
  });
  test("rejects a dynamic_lan timeout that cannot fit within its connection lifetime", () => {
    const snapshot = documents();
    const providers = (snapshot[0].valueJson as { providers: Array<Record<string, unknown>> }).providers;
    providers[0] = { kind: "dynamic-lan", id: "lan-llm-dynamic", enabled: true, label: "dynamic-lan", location: "local", host: "10.0.0.42" };
    const route = (snapshot[2].valueJson as { conversationRespond: { primaryProviderId: string; timeoutMs: number } }).conversationRespond;
    route.primaryProviderId = "lan-llm-dynamic";
    route.timeoutMs = 270_000;
    expect(() => validateSettingsDocuments(snapshot)).toThrow("dynamic LAN conversation timeout must not exceed 269999 ms");
    route.timeoutMs = 269_999;
    expect(() => validateSettingsDocuments(snapshot)).not.toThrow();
  });
  test("rejects unsafe ids and keeps exact provider ids distinct", () => {
    const unsafe = documents();
    (unsafe[0].valueJson as { providers: Array<{ id: string }> }).providers[0].id = "local provider";
    expect(() => validateSettingsDocuments(unsafe)).toThrow("Provider ids may contain only");
    const ambiguous = documents();
    const providers = (ambiguous[0].valueJson as { providers: Array<Record<string, unknown>> }).providers;
    providers[0].id = "local-a";
    providers.push({ kind: "openai-compatible", id: "local_a", enabled: false, label: "Local duplicate", location: "local", endpoint: "", model: "", authentication: "none" });
    (ambiguous[2].valueJson as { conversationRespond: { primaryProviderId: string } }).conversationRespond.primaryProviderId = "local-a";
    expect(() => validateSettingsDocuments(ambiguous)).not.toThrow();
  });
  test("requires an address whenever any service uses the Harness", () => {
    const snapshot = documents();
    (snapshot[0].valueJson as { harness: { address: string } }).harness.address = "";
    expect(() => validateSettingsDocuments(snapshot)).toThrow("Harness address is required");
  });
  test("rejects cloud fallback behind a local-only primary", () => {
    const snapshot = documents();
    const providers = (snapshot[0].valueJson as { providers: Array<Record<string, unknown>> }).providers;
    providers.push({ kind: "openai-compatible", id: "cloud", enabled: true, label: "Cloud", location: "cloud", endpoint: "https://example.com/v1", model: "test", authentication: "api-key" });
    (snapshot[2].valueJson as { conversationRespond: { fallbackProviderIds: string[] } }).conversationRespond.fallbackProviderIds = ["cloud"];
    expect(() => validateSettingsDocuments(snapshot)).toThrow("Cloud fallback is blocked");
  });
  test("rejects duplicate fallback providers", () => {
    const snapshot = documents();
    const providers = (snapshot[0].valueJson as { providers: Array<Record<string, unknown>> }).providers;
    providers.push({ kind: "openai-compatible", id: "fallback", enabled: true, label: "Fallback", location: "local", endpoint: "http://127.0.0.1:11435/v1", model: "local", authentication: "none" });
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
  test("accepts schema 12 and rejects schema 11", () => {
    expect(() => validateSettingsDocuments(documents())).not.toThrow();
    const legacy = documents();
    legacy[0].schemaVersion = 11;
    expect(() => validateSettingsDocuments(legacy)).toThrow("Invalid input");
  });
  test("accepts only the fixed LARM security contract", () => {
    const snapshot = documents();
    const providers = (snapshot[0].valueJson as { providers: Array<Record<string, unknown>> }).providers;
    providers.push({ kind: "larm", id: "larm-local", enabled: false, label: "LARM", location: "local", baseUrl: "http://127.0.0.1:9810", tokenEnv: "LARM_API_TOKEN", allocationTtlSeconds: 300, allocationStartupTimeoutSeconds: 300, allowFallbackByDefault: false, deploymentPolicy: "existing-only" });
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
