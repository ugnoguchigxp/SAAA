export type {
  ConversationMessage,
  ConversationVoicePolicySnapshot,
  RuntimeEvent,
  RuntimeFailureCode,
  VoicePresentationDecision,
} from "./generated/runtimeEvent";
import type { RuntimeEvent as GeneratedRuntimeEvent } from "./generated/runtimeEvent";
export type * from "./settingsTypes";

export type TaskMode = "conversation" | "coding";

export type WebSocketConnectionState = Extract<
  GeneratedRuntimeEvent,
  { type: "webSocketStateChanged" }
>["state"];

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
  schemaVersion: 14;
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

export type LocalArtifactResult = {
  path: string;
  createdAt: string;
};

export type AuditEvent = {
  sequence: number;
  id: string;
  occurredAt: string;
  component: "app" | "frontend" | "microphone" | "voice-asr" | "conversation" | "provider" | "tts" | "meeting" | "settings" | "voice-policy" | "situation";
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
export type TranscriptionScope = "all-speakers" | "target-speaker";
export type MeetingSnapshot = { sessionId: string | null; state: MeetingState; captureToken: string | null; entries: number; transcriptionScope: TranscriptionScope; capabilities: MeetingCapabilities; error: MeetingError | null };
export type MeetingPreflightResult = { state: MeetingState; microphone: { status: string; message: string }; systemAudio: { status: string; message: string }; stt: { status: string; message: string }; translation: { status: string; message: string }; shippingCapabilities: MeetingCapabilities; transcriptionScope: TranscriptionScope; blockingErrors: MeetingError[] };
export type MeetingSegmentResult = { accepted: boolean; text: string; language: string | null };
export type MeetingEvent =
  | { type: "stateChanged"; sessionId: string | null; state: MeetingState }
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
