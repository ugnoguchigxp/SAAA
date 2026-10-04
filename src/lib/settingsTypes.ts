export type {
  DisplayLanguagePreference,
  LengthUnitSystem,
  WeightUnit,
  CurrencyCode,
  RegionalPreferencesSettings,
} from "./regionalSettingsTypes";
export type LlmRequestOptions = {
  tokenLimit: "auto" | "legacy" | "completion";
  reasoning: "auto" | "supported" | "unsupported";
  tools: boolean;
  streaming: boolean;
  thinking?: "auto" | "disabled" | "enabled";
};
import type { AsrLanguageCode } from "./asrLanguages";

export type OpenAiCompatibleProviderSettings = {
  kind: "openai-compatible";
  requestOptions?: LlmRequestOptions;
  id: string;
  enabled: boolean;
  label: string;
  location: "local" | "cloud";
  endpoint: string;
  model: string;
  authentication: "none" | "api-key";
};

export type AgentSessionProviderSettings = {
  kind: "agent-session";
  id: string;
  enabled: boolean;
  label: string;
  location: "local" | "cloud";
  baseUrl: string;
  model: string;
  modelsPath: string;
  sessionsPath: string;
  authentication: "none" | "api-key";
};

export type CloudAsrProviderSettings = {
  kind: "cloud-asr";
  transport?: "http" | "qwen-realtime";
  id: string;
  enabled: boolean;
  label: string;
  location: "local" | "cloud";
  endpoint: string;
  model: string;
  language: "auto";
  authentication: "none" | "api-key";
};

export type CloudTtsProviderSettings = {
  kind: "cloud-tts";
  responseFormat?: "wav" | "pcm";
  id: string;
  enabled: boolean;
  label: string;
  location: "local" | "cloud";
  endpoint: string;
  model: string;
  voice: string;
  authentication: "none" | "api-key";
  style?: string;
  speed?: number;
  pitchScale?: number;
  intonationScale?: number;
};

export type SystemTtsProviderSettings = {
  kind: "system-tts";
  id: string;
  enabled: boolean;
  label: string;
  location: "local";
  voice: string;
};

export type DynamicLanProviderSettings = {
  kind: "dynamic-lan";
  requestOptions?: LlmRequestOptions;
  id: string;
  enabled: boolean;
  label: string;
  location: "local";
  host: string;
};

export type ModelProviderSettings =
  | OpenAiCompatibleProviderSettings
  | AgentSessionProviderSettings
  | CloudAsrProviderSettings
  | CloudTtsProviderSettings
  | SystemTtsProviderSettings
  | DynamicLanProviderSettings;

export type ReasoningEffort = "provider-default" | "low" | "medium" | "xhigh";

export type ModelProvidersSettings = {
  harness: {
    address: string;
    larmProfile?: string;
    ttsVoice?: string;
    ttsStyle?: string;
    ttsSpeed?: number;
    ttsPitchScale?: number;
    ttsIntonationScale?: number;
  };
  providers: ModelProviderSettings[];
  reasoningEffort: ReasoningEffort;
};

export type CodexAgentSettings = {
  agentName: string;
  userName: string;
  enabled: boolean;
  provider: "codex-sdk";
  model: string;
  runtimeMode: "pending-compatibility-check" | "bun" | "node-sidecar" | "app-server";
  health: "unchecked" | "ready" | "unavailable";
  sandboxMode: "read-only";
  approvalPolicy: "never";
  networkEnabled: false;
  webSearchEnabled: false;
  workspacePolicy: "select-per-conversation";
};

export type RoutingSettings = {
  conversationRespond: {
    source: "harness" | "provider";
    primaryProviderId: string | null;
    fallbackProviderIds: string[];
    attemptTimeoutMs?: number;
    timeoutMs: number;
  };
  voiceTranscribe: {
    source: "harness" | "provider";
    providerId: string | null;
    timeoutMs: number;
    attemptTimeoutMs?: number;
    fallbackProviderIds?: string[];
  };
  voiceSpeak: {
    source: "harness" | "provider";
    providerId: string | null;
    timeoutMs: number;
    attemptTimeoutMs?: number;
    fallbackProviderIds?: string[];
  };
  codingAssist: {
    providerId: "codex-sdk";
    timeoutMs: number;
    readOnly: true;
    networkEnabled: false;
    webSearchEnabled: false;
  };
};

export type VoiceSettings = {
  listeningEnabled: boolean;
  inputDeviceId: string;
  outputDeviceId: string;
  vadSensitivity: "low" | "medium" | "high";
  silenceTimeoutMs: number;
  allowedLanguages: AsrLanguageCode[];
  autoSpeak: boolean;
  aecEnabled: boolean;
  otherAudioDucking: "default" | "min" | "mid" | "max";
  vpioOnBluetooth: boolean;
  bargeInEnabled: boolean;
};

export type SecuritySettings = { localOnlyWhenSelected: boolean; diagnosticsRedaction: boolean };
export type SituationSettings = {
  enabled: boolean;
  sampleIntervalMs: number;
  calendarEnabled: boolean;
  retentionDays: number;
  maxLedgerEntries: number;
  heartbeatIntervalMs: number;
  sensitiveApplicationCategories: true;
};

export type { RoleRoutingSettings } from "./roleRoutingTypes";
