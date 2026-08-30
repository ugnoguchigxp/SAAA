import type { SettingsDraft } from "./settingsDraft";

export const DEFAULT_AGENT_NAME = "SAAA";
export const DEFAULT_DYNAMIC_LAN_HOST = "localhost";
export const DYNAMIC_LAN_PROVIDER_ID = "lan-llm-dynamic";

export const defaultSettingsDraft: SettingsDraft = {
  providers: {
    harness: { address: `http://${DEFAULT_DYNAMIC_LAN_HOST}:9810` },
    providers: [
      {
        kind: "dynamic-lan",
        id: DYNAMIC_LAN_PROVIDER_ID,
        enabled: true,
        label: "Provider Harness LLM",
        location: "local",
        host: DEFAULT_DYNAMIC_LAN_HOST,
      },
      {
        kind: "system-tts",
        id: "system-tts",
        enabled: true,
        label: "System Voice",
        location: "local",
        voice: "default",
      },
    ],
    reasoningEffort: "medium",
  },
  codex: {
    agentName: DEFAULT_AGENT_NAME,
    userName: "",
    enabled: false,
    provider: "codex-sdk",
    model: "",
    runtimeMode: "app-server",
    health: "unchecked",
    sandboxMode: "read-only",
    approvalPolicy: "never",
    networkEnabled: false,
    webSearchEnabled: false,
    workspacePolicy: "select-per-conversation",
  },
  routing: {
    conversationRespond: {
      source: "harness",
      primaryProviderId: null,
      fallbackProviderIds: [],
      timeoutMs: 30_000,
    },
    voiceTranscribe: { source: "harness", providerId: null, timeoutMs: 120_000 },
    voiceSpeak: { source: "provider", providerId: "system-tts", timeoutMs: 30_000 },
    codingAssist: {
      providerId: "codex-sdk",
      timeoutMs: 120_000,
      readOnly: true,
      networkEnabled: false,
      webSearchEnabled: false,
    },
  },
  voice: {
    listeningEnabled: true,
    inputDeviceId: "default",
    outputDeviceId: "default",
    vadSensitivity: "medium",
    silenceTimeoutMs: 1_500,
    allowedLanguages: ["ja"],
    autoSpeak: true,
  },
  security: {
    localOnlyWhenSelected: true,
    diagnosticsRedaction: true,
  },
  situation: {
    enabled: false,
    sampleIntervalMs: 2_000,
    calendarEnabled: false,
    retentionDays: 7,
    maxLedgerEntries: 10_000,
    heartbeatIntervalMs: 300_000,
    sensitiveApplicationCategories: true,
  },
};
