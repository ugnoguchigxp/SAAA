import { appSnapshotSchema, parseIpc } from "./ipcValidation";
import { invoke } from "@tauri-apps/api/core";
import { stageAudioUpload } from "./audioIpc";
import type {
  AppSnapshot,
  AuditEvent,
  AuditEventListInput,
  ConversationMessage,
  ModelProviderSettings,
  LocalArtifactResult,
  ProviderTestResult,
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

export type TtsVoiceCatalog = {
  defaultVoice?: string;
  voices: Array<{
    id: string;
    displayName: string;
    voicePresentation?: string;
    defaultStyle?: string;
    styles: Array<{ id: string; displayName: string }>;
    credit?: string;
  }>;
};

export async function loadTtsVoiceCatalog(input: {
  source: "provider" | "harness";
  providerId?: string;
}): Promise<TtsVoiceCatalog> {
  return invoke<TtsVoiceCatalog>("load_tts_voice_catalog", { input });
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

export async function listAuditEvents(input: AuditEventListInput): Promise<AuditEvent[]> {
  return invoke<AuditEvent[]>("list_audit_events", { input });
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

export async function prepareComposerImage(png: Uint8Array): Promise<{
  id: string;
  width: number;
  height: number;
  byteLength: number;
  preview: number[];
}> {
  return invoke("prepare_composer_image", png);
}

export async function discardComposerImage(imageId: string): Promise<void> {
  return invoke("discard_composer_image", { imageId });
}

export async function claimTurnImage(imageId: string, runId: string): Promise<void> {
  return invoke("claim_turn_image", { imageId, runId });
}

export async function releaseTurnImage(runId: string): Promise<void> {
  return invoke("release_turn_image", { runId });
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
