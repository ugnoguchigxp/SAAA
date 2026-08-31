export type {
  ConversationMessage,
  ConversationVoicePolicySnapshot,
  RuntimeEvent,
  RuntimeFailureCode,
  VoicePresentationDecision,
} from "./generated/runtimeEvent";
import { isAsrLanguageCode } from "./asrLanguages";
import {
  CURRENCY_CODES,
  DISPLAY_LANGUAGE_PREFERENCES,
  isSupportedTimeZone,
  LENGTH_UNIT_SYSTEMS,
  WEIGHT_UNITS,
} from "./regionalPreferences";
import type {
  CodexAgentSettings,
  ModelProviderSettings,
  ModelProvidersSettings,
  RoutingSettings,
  RegionalPreferencesSettings,
  SecuritySettings,
  SituationSettings,
  VoiceSettings,
} from "./settingsTypes";
export type * from "./settingsTypes";

export type TaskMode = "conversation" | "coding";

export type SettingsNamespace =
  | "providers.model"
  | "providers.agent"
  | "routing.tasks"
  | "voice.runtime"
  | "security.runtime"
  | "ui.preferences"
  | "situation.runtime";

export type SettingsKey = "default" | "codex-sdk";

export type SettingsDocument = {
  namespace: SettingsNamespace;
  key: SettingsKey;
  schemaVersion: 13;
  valueJson: Record<string, unknown>;
  updatedAt: string;
};

export type Conversation = {
  id: string;
  title: string | null;
  taskMode: TaskMode;
  createdAt: string;
  updatedAt: string;
};

export type AppSnapshot = {
  settings: SettingsDocument[];
  conversations: Conversation[];
  primaryConversationId: string;
  effectiveRoute: EffectiveRouteSnapshot;
  larmRuntime: LarmRuntimeStatus;
  voiceProfile: VoiceProfileSnapshot;
};

export type EffectiveRouteSnapshot = {
  providerId: string | null;
  label: string;
  location: "local" | "cloud" | null;
  state: "unchecked" | "active" | "ready" | "failed";
  fallbackUsed: boolean;
  reasonCode: string;
  updatedAt: string | null;
};

export type VoiceProfileSnapshot = {
  status: "empty" | "collecting" | "ready";
  filterEnabled: boolean;
  runtimeAvailable: boolean;
  runtimeMessage: string;
  sampleCount: number;
  targetSampleCount: number;
  totalDurationMs: number;
  minimumDurationMs: number;
  threshold: number;
  samples: VoiceSampleSummary[];
};

export type VoiceSampleSummary = {
  id: string;
  ordinal: number;
  durationMs: number;
  inputDeviceId: string;
  effectiveAec: boolean;
  createdAt: string;
};

export type LarmRuntimeStatus = {
  state: "disabled" | "ready" | "unavailable";
  message: string;
  contractCommit: string;
};

export type ProviderTestResult = {
  providerId: string;
  ok: boolean;
  message: string;
  latencyMs: number;
};

export type ProviderCredentialState = {
  providerId: string;
  state: "configured" | "missing" | "unavailable";
};

export type HarnessServiceStatus = {
  capability: "llm" | "asr" | "tts";
  state: "ready" | "unavailable";
  protocol: string | null;
  model: string | null;
  language: string | null;
  voice: string | null;
  message: string;
};

export type HarnessResolution = {
  state: "ready" | "degraded";
  revision: string;
  services: HarnessServiceStatus[];
};

export type NetworkAsrResolution = {
  providerId: "network-asr";
  endpoint: string;
  model: string;
};

export type LocalArtifactResult = {
  path: string;
  createdAt: string;
};

export type SignalHealth = "ready" | "disabled" | "permission-denied" | "unsupported" | "degraded";
export type ForegroundCategory = "communication" | "coding" | "writing" | "browser" | "media" | "sensitive" | "other" | "unknown";
export type ConversationSignalState = "idle" | "user-input" | "model-running" | "agent-running";
export type MicrophoneSignalState = "inactive" | "saaa-capturing" | "saaa-transcribing" | "external-active" | "unknown";
export type AudioSignalState = "silent" | "saaa-speaking" | "external-media" | "unknown";
export type InputActivityState = "active" | "recent" | "idle" | "unknown";

export type SignalSnapshot = {
  sequence: number;
  observedAt: string;
  foreground: { category: ForegroundCategory; health: SignalHealth };
  conversation: { state: ConversationSignalState };
  microphone: { state: MicrophoneSignalState; health: SignalHealth };
  audio: { state: AudioSignalState; health: SignalHealth };
  calendar: {
    state: "free" | "busy" | "meeting-likely" | "unavailable";
    timeBucket: "now" | "within-15m" | "later" | "none";
    health: SignalHealth;
  };
  inputActivity: { state: InputActivityState; health: SignalHealth };
};

export type SituationState = {
  scene: string;
  confidence: number;
  userAttention: "available" | "busy" | "unknown";
  audioEnvironment: "silence" | "speech" | "multi-speaker" | "media" | "unknown";
  evidence: Array<{ code: string; weight: number }>;
  candidateSince: string;
  stableSince: string;
  updatedAt: string;
  ruleVersion: string;
};

export type ShadowDecision = {
  mode: "shadow";
  proposedAttention: "IGNORE" | "OBSERVE" | "SUGGEST" | "RESPOND";
  actualExecution: "NONE";
  actualPresentation: "SILENT";
  reasonCodes: string[];
  decidedAt: string;
  policyVersion: string;
};

export type SituationFeedback = {
  verdict: "accurate" | "inaccurate" | "unsure";
  impact: "none" | "no-effect" | "harmful";
  correctedScene: string | null;
  reasonCode: string | null;
  createdAt: string;
};

export type CalibrationParameters = { classificationMinConfidence: number; lowConfidenceMax: number; enterSampleCount: number; exitSampleCount: number; cooldownMs: number; inputActiveMaxMs: number; inputRecentMaxMs: number };
export type CalibrationProfile = { id: string; ruleVersion: string; baseRuleVersion: string | null; status: "candidate" | "active" | "superseded" | "rejected" | "rolled-back"; parameters: CalibrationParameters; createdAt: string; decidedAt: string | null; decisionReasonCode: string | null };
export type CalibrationRun = { id: string; profileId: string; fixtureSetVersion: string; status: "completed" | "failed"; metricsJson: string | null; errorCode: string | null; startedAt: string; completedAt: string };
export type SituationReviewSnapshot = { activeProfile: CalibrationProfile; quality: { sampleCount: number; flappingRate: number | null; staleRate: number | null }; feedbackQueue: SituationLedgerEntry[]; latestRun: CalibrationRun | null; candidates: CalibrationProfile[] };

export type SituationLedgerEntry = {
  id: string;
  observedAt: string;
  state: SituationState;
  decision: ShadowDecision;
  signalHealth: Array<{ source: string; health: SignalHealth }>;
  entryKind: "transition" | "decision" | "heartbeat";
  feedback: SituationFeedback | null;
};

export type SituationSnapshot = {
  monitoringEnabled: boolean;
  monitoringActive: boolean;
  signals: SignalSnapshot;
  state: SituationState;
  decision: ShadowDecision;
  lastFailure: { code: string; message: string; recovery: string } | null;
  history: SituationLedgerEntry[];
  evaluation: { totalEntries: number; accurate: number; inaccurate: number; unsure: number };
};

export type SituationEvent =
  | { type: "signalHealthChanged"; source: string; health: SignalHealth }
  | { type: "candidateChanged"; state: SituationState }
  | { type: "stableStateChanged"; entry: SituationLedgerEntry }
  | { type: "shadowDecisionChanged"; entry: SituationLedgerEntry }
  | { type: "monitoringStopped"; reason: string }
  | { type: "failed"; code: string; message: string; recovery: string };

export type MeetingState = "idle" | "preflight" | "ready" | "active" | "paused" | "stopping" | "completed" | "failed";
export type MeetingLane = "microphone" | "system-audio";
export type MeetingCapabilities = { microphone: boolean; systemAudio: boolean; overlay: boolean; translation: boolean };
export type MeetingError = { code: string; message: string; recovery: string };
export type MeetingSnapshot = { sessionId: string | null; state: MeetingState; captureToken: string | null; entries: number; capabilities: MeetingCapabilities; error: MeetingError | null };
export type MeetingPreflightResult = { state: MeetingState; microphone: { status: string; message: string }; systemAudio: { status: string; message: string }; stt: { status: string; message: string }; translation: { status: string; message: string }; shippingCapabilities: MeetingCapabilities; blockingErrors: MeetingError[] };
export type MeetingSegmentResult = { accepted: boolean; text: string; language: string | null };
export type MeetingEvent =
  | { type: "stateChanged"; sessionId: string | null; state: MeetingState }
  | { type: "transcriptPartial"; sessionId: string; lane: MeetingLane; sequence: number; text: string; language: string | null }
  | { type: "transcriptFinal"; sessionId: string; lane: MeetingLane; sequence: number; text: string; language: string | null }
  | { type: "failed"; sessionId: string | null; code: string; message: string; recovery: string };

export function findSettingsDocument(
  documents: SettingsDocument[],
  namespace: SettingsNamespace,
  key: SettingsKey,
): SettingsDocument | undefined {
  return documents.find(
    (document) => document.namespace === namespace && document.key === key,
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function isModelProvidersSettings(value: Record<string, unknown>): value is ModelProvidersSettings {
  return (
    isRecord(value.harness) &&
    typeof value.harness.address === "string" &&
    Array.isArray(value.providers) &&
    value.providers.every(isModelProviderSettings) &&
    (value.reasoningEffort === "low" || value.reasoningEffort === "medium" || value.reasoningEffort === "xhigh")
  );
}

function isModelProviderSettings(value: unknown): value is ModelProviderSettings {
  if (!isRecord(value)) return false;
  const common = (
    typeof value.id === "string" &&
    typeof value.enabled === "boolean" &&
    typeof value.label === "string" &&
    (value.location === "local" || value.location === "cloud")
  );
  if (!common) return false;
  if (value.kind === "openai-compatible") {
    return (
      typeof value.endpoint === "string" &&
      typeof value.model === "string" &&
      (value.authentication === "none" || value.authentication === "api-key")
    );
  }
  if (value.kind === "cloud-asr") {
    return (
      value.location === "cloud" &&
      typeof value.endpoint === "string" &&
      typeof value.model === "string" &&
      value.language === "auto" &&
      (value.authentication === "none" || value.authentication === "api-key")
    );
  }
  if (value.kind === "cloud-tts") {
    return (
      value.location === "cloud" &&
      typeof value.endpoint === "string" &&
      typeof value.model === "string" &&
      typeof value.voice === "string" &&
      (value.authentication === "none" || value.authentication === "api-key")
    );
  }
  if (value.kind === "system-tts") {
    return value.location === "local" && typeof value.voice === "string";
  }
  if (value.kind === "dynamic-lan") {
    return value.location === "local" && typeof value.host === "string";
  }
  return (
    value.kind === "larm" &&
    value.location === "local" &&
    typeof value.baseUrl === "string" &&
    value.tokenEnv === "LARM_API_TOKEN" &&
    typeof value.allocationTtlSeconds === "number" &&
    typeof value.allocationStartupTimeoutSeconds === "number" &&
    value.allowFallbackByDefault === false &&
    value.deploymentPolicy === "existing-only"
  );
}

export function isCodexAgentSettings(value: Record<string, unknown>): value is CodexAgentSettings {
  return (
    typeof value.agentName === "string" &&
    typeof value.userName === "string" &&
    typeof value.enabled === "boolean" &&
    value.provider === "codex-sdk" &&
    typeof value.model === "string" &&
    (value.runtimeMode === "pending-compatibility-check" || value.runtimeMode === "bun" || value.runtimeMode === "node-sidecar" || value.runtimeMode === "app-server") &&
    (value.health === "unchecked" || value.health === "ready" || value.health === "unavailable") &&
    value.sandboxMode === "read-only" &&
    value.approvalPolicy === "never" &&
    value.networkEnabled === false &&
    value.webSearchEnabled === false &&
    value.workspacePolicy === "select-per-conversation"
  );
}

export function isRoutingSettings(value: Record<string, unknown>): value is RoutingSettings {
  if (
    !isRecord(value.conversationRespond) ||
    !isRecord(value.voiceTranscribe) ||
    !isRecord(value.voiceSpeak) ||
    !isRecord(value.codingAssist)
  ) return false;
  const conversation = value.conversationRespond;
  const transcribe = value.voiceTranscribe;
  const speak = value.voiceSpeak;
  const coding = value.codingAssist;
  return (
    (conversation.source === "harness" || conversation.source === "provider") &&
    (conversation.primaryProviderId === null || typeof conversation.primaryProviderId === "string") &&
    Array.isArray(conversation.fallbackProviderIds) &&
    conversation.fallbackProviderIds.every((id) => typeof id === "string") &&
    typeof conversation.timeoutMs === "number" &&
    (transcribe.source === "harness" || transcribe.source === "provider") &&
    (transcribe.providerId === null || typeof transcribe.providerId === "string") &&
    typeof transcribe.timeoutMs === "number" &&
    (speak.source === "harness" || speak.source === "provider") &&
    (speak.providerId === null || typeof speak.providerId === "string") &&
    typeof speak.timeoutMs === "number" &&
    coding.providerId === "codex-sdk" &&
    typeof coding.timeoutMs === "number" &&
    coding.readOnly === true &&
    coding.networkEnabled === false &&
    coding.webSearchEnabled === false
  );
}

export function isVoiceSettings(value: Record<string, unknown>): value is VoiceSettings {
  return (
    typeof value.listeningEnabled === "boolean" &&
    typeof value.inputDeviceId === "string" &&
    typeof value.outputDeviceId === "string" &&
    (value.vadSensitivity === "low" || value.vadSensitivity === "medium" || value.vadSensitivity === "high") &&
    typeof value.silenceTimeoutMs === "number" &&
    Number.isInteger(value.silenceTimeoutMs) &&
    value.silenceTimeoutMs >= 800 &&
    value.silenceTimeoutMs <= 3000 &&
    Array.isArray(value.allowedLanguages) &&
    value.allowedLanguages.length > 0 &&
    new Set(value.allowedLanguages).size === value.allowedLanguages.length &&
    value.allowedLanguages.every(isAsrLanguageCode) &&
    typeof value.autoSpeak === "boolean"
  );
}

export function isSecuritySettings(value: Record<string, unknown>): value is SecuritySettings {
  return (
    typeof value.localOnlyWhenSelected === "boolean" &&
    value.diagnosticsRedaction === true
  );
}

export function isRegionalPreferencesSettings(value: Record<string, unknown>): value is RegionalPreferencesSettings {
  return (
    DISPLAY_LANGUAGE_PREFERENCES.some((language) => language === value.language) &&
    typeof value.timeZone === "string" &&
    isSupportedTimeZone(value.timeZone) &&
    LENGTH_UNIT_SYSTEMS.some((unit) => unit === value.lengthUnit) &&
    WEIGHT_UNITS.some((unit) => unit === value.weightUnit) &&
    CURRENCY_CODES.some((currency) => currency === value.currency)
  );
}

export function isSituationSettings(value: Record<string, unknown>): value is SituationSettings {
  return (
    typeof value.enabled === "boolean" &&
    typeof value.sampleIntervalMs === "number" &&
    Number.isInteger(value.sampleIntervalMs) &&
    value.sampleIntervalMs >= 500 && value.sampleIntervalMs <= 60_000 &&
    typeof value.calendarEnabled === "boolean" &&
    typeof value.retentionDays === "number" &&
    Number.isInteger(value.retentionDays) &&
    value.retentionDays >= 1 && value.retentionDays <= 30 &&
    typeof value.maxLedgerEntries === "number" &&
    Number.isInteger(value.maxLedgerEntries) &&
    value.maxLedgerEntries >= 100 && value.maxLedgerEntries <= 10_000 &&
    typeof value.heartbeatIntervalMs === "number" &&
    Number.isInteger(value.heartbeatIntervalMs) &&
    value.heartbeatIntervalMs >= 60_000 && value.heartbeatIntervalMs <= 3_600_000 &&
    value.sensitiveApplicationCategories === true
  );
}
