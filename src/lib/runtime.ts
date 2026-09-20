import { runtimeEventOrder } from "./ipcEventOrder";
import { appSnapshotSchema, runtimeEventSchema, parseIpc, guardedReceiver } from "./ipcValidation";
import { Channel, invoke } from "@tauri-apps/api/core";
import { stageAudioUpload } from "./audioIpc";
import type {
  AppSnapshot,
  AuditEvent,
  ConversationMessage,
  ModelProviderSettings,
  LocalArtifactResult,
  ProviderTestResult,
  RuntimeEvent,
  SettingsDocument,
  VoiceProfileSnapshot,
} from "./contracts";
import { validateSettingsDocuments } from "./schemas";
export {
  deleteProviderApiKey,
  getProviderCredentialState,
  resolveServiceHarness,
  setProviderApiKey,
} from "./providerRuntime";
export {
  appendVoiceAsrAudio,
  commitVoiceAsrUtterance,
  startVoiceAsrSession,
  stopVoiceAsrSession,
} from "./voiceAsrRuntime";

export async function startTurn(
  input: {
    runId: string;
    conversationId: string;
    content: string;
    workspacePath: string | null;
    retryInputMessageId?: string | null;
    scopeRefs?: Array<{
      kind: "user" | "project" | "task" | "resource" | "request";
      id: string;
      relation: "shared" | "parent" | "focus" | "current";
    }>;
    sourceId?: string | null;
    inputOrigin: "text" | "voice";
    presentationMode: "visual" | "visual-and-spoken";
  },
  onEvent: (event: RuntimeEvent) => void,
): Promise<void> {
  const channel = new Channel<unknown>();
  channel.onmessage = guardedReceiver(
    runtimeEventSchema,
    "runtime",
    onEvent,
    runtimeEventOrder(input.runId),
    () => {
      void cancelRun(input.runId).catch(() => undefined);
    },
  );
  return invoke<void>("start_turn", { input, onEvent: channel });
}

export async function cancelRun(runId: string): Promise<void> {
  return invoke<void>("cancel_run", { runId });
}

export async function testModelProvider(
  provider: ModelProviderSettings,
): Promise<ProviderTestResult> {
  return invoke<ProviderTestResult>("test_model_provider", { input: { provider } });
}

export async function stopTts(runId: string): Promise<void> {
  return invoke<void>("stop_tts", { runId });
}

export async function getAppSnapshot(): Promise<AppSnapshot> {
  return parseIpc(appSnapshotSchema, await invoke<unknown>("get_app_snapshot"), "snapshot");
}

export async function getVoiceProfileSnapshot(): Promise<VoiceProfileSnapshot> {
  return invoke<VoiceProfileSnapshot>("get_voice_profile_snapshot");
}

export async function saveVoiceEnrollmentSample(input: {
  samples: Float32Array;
  sampleRate: number;
  inputDeviceId: string;
  effectiveAec: boolean;
}): Promise<VoiceProfileSnapshot> {
  const { samples, ...metadata } = input;
  const audioUploadId = await stageAudioUpload(samples, "voice-enrollment").finally(() =>
    samples.fill(0),
  );
  return invoke<VoiceProfileSnapshot>("save_voice_enrollment_sample", {
    input: { ...metadata, audioUploadId },
  });
}

export async function setTargetSpeakerFilterEnabled(
  enabled: boolean,
): Promise<VoiceProfileSnapshot> {
  return invoke<VoiceProfileSnapshot>("set_target_speaker_filter_enabled", { input: { enabled } });
}

export async function deleteVoiceEnrollmentSample(sampleId: string): Promise<VoiceProfileSnapshot> {
  return invoke<VoiceProfileSnapshot>("delete_voice_enrollment_sample", { sampleId });
}

export async function deleteVoiceProfile(): Promise<VoiceProfileSnapshot> {
  return invoke<VoiceProfileSnapshot>("delete_voice_profile");
}

export async function readVoiceEnrollmentSample(sampleId: string): Promise<ArrayBuffer> {
  return invoke<ArrayBuffer>("read_voice_enrollment_sample", { sampleId });
}

export async function reportFrontendReady(): Promise<void> {
  return invoke<void>("frontend_ready");
}

export async function exportDiagnostics(): Promise<LocalArtifactResult> {
  return invoke<LocalArtifactResult>("export_diagnostics");
}

export async function listAuditEvents(): Promise<AuditEvent[]> {
  return invoke<AuditEvent[]>("list_audit_events");
}

export async function backupDatabase(): Promise<LocalArtifactResult> {
  return invoke<LocalArtifactResult>("backup_database");
}

export async function saveSettingsDocuments(
  documents: Array<Omit<SettingsDocument, "updatedAt">>,
): Promise<SettingsDocument[]> {
  validateSettingsDocuments(documents);
  return invoke<SettingsDocument[]>("save_settings_documents", {
    input: { documents },
  });
}

export async function setVoiceListeningEnabled(enabled: boolean): Promise<SettingsDocument> {
  return invoke<SettingsDocument>("set_voice_listening_enabled", { input: { enabled } });
}

export async function listMessages(
  conversationId: string,
  cursor: string | null,
): Promise<{ messages: ConversationMessage[]; hasMore: boolean; nextCursor: string | null }> {
  return invoke<{ messages: ConversationMessage[]; hasMore: boolean; nextCursor: string | null }>(
    "list_messages",
    {
      input: { conversationId, cursor },
    },
  );
}

export async function reportOwnedSignal(input: {
  conversationState: "idle" | "user-input" | "model-running" | "agent-running";
  microphoneState:
    | "inactive"
    | "saaa-capturing"
    | "saaa-transcribing"
    | "external-active"
    | "unknown";
  audioState: "silent" | "saaa-speaking" | "external-media" | "unknown";
}): Promise<void> {
  return invoke<void>("report_owned_signal", { input });
}
