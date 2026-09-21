export type {
  ConversationMessage,
  ConversationVoicePolicySnapshot,
  RoutingEventRecord,
  RoutingRootSnapshot,
  RoutingSnapshot,
  RuntimeEvent,
  RuntimeFailureCode,
  VoicePresentationDecision,
} from "./generated/runtimeEvent";
export type * from "./settingsTypes";

export type TaskMode = "conversation" | "coding";

export type SettingsNamespace =
  | "providers.model"
  | "providers.agent"
  | "routing.tasks"
  | "voice.runtime"
  | "security.runtime"
  | "ui.preferences"
  | "situation.runtime"
  | "routing.roles";

export type SettingsKey = "default" | "codex-sdk";

export type SettingsDocument = {
  namespace: SettingsNamespace;
  key: SettingsKey;
  schemaVersion: 1 | 15;
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

export type LocalArtifactResult = {
  path: string;
  createdAt: string;
};

export type AuditEvent = {
  sequence: number;
  id: string;
  occurredAt: string;
  component:
    | "app"
    | "frontend"
    | "microphone"
    | "voice-asr"
    | "conversation"
    | "provider"
    | "tts"
    | "settings"
    | "voice-policy"
    | "situation";
  eventName: string;
  phase: "request" | "start" | "state" | "progress" | "decision" | "terminal" | "error";
  outcome: "success" | "failure" | "cancelled" | "interrupted" | "degraded" | "blocked" | null;
  correlationId: string | null;
  causationId: string | null;
  conversationId: string | null;
  runtimeRunId: string | null;
  sessionId: string | null;
  subjectId: string | null;
  failureCode: string | null;
  attributes: Record<string, boolean | number | string>;
};

export type AuditEventSortField =
  | "occurredAt"
  | "component"
  | "eventName"
  | "phase"
  | "outcome"
  | "failureCode";

export type AuditEventListInput = {
  sortBy: AuditEventSortField;
  direction: "asc" | "desc";
};

export function findSettingsDocument(
  documents: SettingsDocument[],
  namespace: SettingsNamespace,
  key: SettingsKey,
): SettingsDocument | undefined {
  return documents.find((document) => document.namespace === namespace && document.key === key);
}
