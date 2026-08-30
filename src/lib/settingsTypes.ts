import type { AsrLanguageCode } from "./asrLanguages";

export type OpenAiCompatibleProviderSettings = {
  kind: "openai-compatible";
  id: string;
  enabled: boolean;
  label: string;
  location: "local" | "cloud";
  endpoint: string;
  model: string;
  authentication: "none" | "api-key";
};

export type CloudAsrProviderSettings = {
  kind: "cloud-asr";
  id: string;
  enabled: boolean;
  label: string;
  location: "cloud";
  endpoint: string;
  model: string;
  language: "auto";
  authentication: "none" | "api-key";
};

export type CloudTtsProviderSettings = {
  kind: "cloud-tts";
  id: string;
  enabled: boolean;
  label: string;
  location: "cloud";
  endpoint: string;
  model: string;
  voice: string;
  authentication: "none" | "api-key";
};

export type SystemTtsProviderSettings = {
  kind: "system-tts";
  id: string;
  enabled: boolean;
  label: string;
  location: "local";
  voice: string;
};

export type LarmProviderSettings = {
  kind: "larm";
  id: string;
  enabled: boolean;
  label: string;
  location: "local";
  baseUrl: string;
  tokenEnv: "LARM_API_TOKEN";
  allocationTtlSeconds: number;
  allocationStartupTimeoutSeconds: number;
  allowFallbackByDefault: false;
  deploymentPolicy: "existing-only";
};

export type DynamicLanProviderSettings = {
  kind: "dynamic-lan";
  id: string;
  enabled: boolean;
  label: string;
  location: "local";
  host: string;
};

export type ModelProviderSettings =
  | OpenAiCompatibleProviderSettings
  | CloudAsrProviderSettings
  | CloudTtsProviderSettings
  | SystemTtsProviderSettings
  | LarmProviderSettings
  | DynamicLanProviderSettings;

export type ReasoningEffort = "low" | "medium" | "xhigh";

export type ModelProvidersSettings = {
  harness: { address: string };
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

export type CodexReasoningEffort = { reasoningEffort: string; description: string };
export type CodexModelOption = {
  id: string;
  model: string;
  displayName: string;
  description: string;
  hidden: boolean;
  defaultReasoningEffort: string | null;
  supportedReasoningEfforts: CodexReasoningEffort[];
  inputModalities: string[];
  supportsPersonality: boolean;
  isDefault: boolean;
};
export type CodexRuntimeStatus = {
  installed: boolean;
  authenticated: boolean;
  runtime: string;
  accountType: string | null;
  message: string;
};

export type RoutingSettings = {
  conversationRespond: { source: "harness" | "provider"; primaryProviderId: string | null; fallbackProviderIds: string[]; timeoutMs: number };
  voiceTranscribe: { source: "harness" | "provider"; providerId: string | null; timeoutMs: number };
  voiceSpeak: { source: "harness" | "provider"; providerId: string | null; timeoutMs: number };
  codingAssist: { providerId: "codex-sdk"; timeoutMs: number; readOnly: true; networkEnabled: false; webSearchEnabled: false };
};

export type VoiceSettings = {
  listeningEnabled: boolean;
  inputDeviceId: string;
  outputDeviceId: string;
  vadSensitivity: "low" | "medium" | "high";
  silenceTimeoutMs: number;
  allowedLanguages: AsrLanguageCode[];
  autoSpeak: boolean;
};

export type TtsCapabilities = {
  available: boolean;
  message: string;
  voices: Array<{ id: string; label: string; language: string | null }>;
  outputDevices: string[];
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
