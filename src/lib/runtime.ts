import { Channel, invoke } from "@tauri-apps/api/core";
import { stageAudioUpload } from "./audioIpc";
import type {
  AppSnapshot,
  CodexModelOption,
  CodexRuntimeStatus,
  Conversation,
  ConversationMessage,
  ModelProviderSettings,
  NetworkAsrResolution,
  LocalArtifactResult,
  ProviderTestResult,
  RuntimeEvent,
  SettingsDocument,
  SituationSnapshot,
  SituationReviewSnapshot, CalibrationParameters, CalibrationProfile, CalibrationRun,
  TaskMode,
  VoiceProfileSnapshot,
  MeetingPreflightResult,
  MeetingSegmentResult,
  MeetingSnapshot,
  MeetingEvent,
  TtsCapabilities,
} from "./contracts";
import { validateSettingsDocuments } from "./schemas";
export { deleteProviderApiKey, getProviderCredentialState, resolveServiceHarness, setProviderApiKey } from "./providerRuntime";
export { transcribeAudio, transcribeAudioChunk } from "./voiceRuntime";

export async function listCodexModels(): Promise<CodexModelOption[]> {
  return invoke<CodexModelOption[]>("list_codex_models");
}

export async function getCodexStatus(): Promise<CodexRuntimeStatus> {
  return invoke<CodexRuntimeStatus>("get_codex_status");
}

export async function startTurn(
  input: { runId: string; conversationId: string; content: string; workspacePath: string | null; retryInputMessageId?: string | null; inputOrigin: "text" | "voice"; presentationMode: "visual" | "visual-and-spoken" },
  onEvent: (event: RuntimeEvent) => void,
): Promise<void> {
  const channel = new Channel<RuntimeEvent>();
  channel.onmessage = onEvent;
  return invoke<void>("start_turn", { input, onEvent: channel });
}

export async function cancelRun(runId: string): Promise<void> {
  return invoke<void>("cancel_run", { runId });
}

export async function testModelProvider(provider: ModelProviderSettings): Promise<ProviderTestResult> {
  return invoke<ProviderTestResult>("test_model_provider", { input: { provider } });
}

export async function resolveNetworkAsr(host: string): Promise<NetworkAsrResolution> {
  return invoke<NetworkAsrResolution>("resolve_network_asr", { input: { host } });
}

export async function speakText(input: {
  runId: string;
  conversationId: string;
  text: string;
}): Promise<void> {
  return invoke<void>("speak_text", { input });
}

export async function listTtsCapabilities(): Promise<TtsCapabilities> {
  return invoke<TtsCapabilities>("list_tts_capabilities");
}

export async function stopTts(runId: string): Promise<void> {
  return invoke<void>("stop_tts", { runId });
}

export async function getAppSnapshot(): Promise<AppSnapshot> {
  return invoke<AppSnapshot>("get_app_snapshot");
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
  const audioUploadId = await stageAudioUpload(samples, "voice-enrollment").finally(() => samples.fill(0));
  return invoke<VoiceProfileSnapshot>("save_voice_enrollment_sample", {
    input: { ...metadata, audioUploadId },
  });
}

export async function setTargetSpeakerFilterEnabled(enabled: boolean): Promise<VoiceProfileSnapshot> {
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

export async function createConversation(taskMode: TaskMode): Promise<Conversation> {
  return invoke<Conversation>("create_conversation", {
    input: { taskMode, title: null },
  });
}

export async function listMessages(conversationId: string): Promise<ConversationMessage[]> {
  return invoke<ConversationMessage[]>("list_messages", { conversationId });
}

export async function appendMessage(input: {
  conversationId: string;
  role: ConversationMessage["role"];
  content: string;
}): Promise<ConversationMessage> {
  return invoke<ConversationMessage>("append_message", { input });
}

export async function getSituationSnapshot(): Promise<SituationSnapshot> {
  return invoke<SituationSnapshot>("get_situation_snapshot");
}

export async function setSituationMonitoring(enabled: boolean): Promise<SituationSnapshot> {
  return invoke<SituationSnapshot>("set_situation_monitoring", { enabled });
}

export async function reportOwnedSignal(input: {
  conversationState: "idle" | "user-input" | "model-running" | "agent-running";
  microphoneState: "inactive" | "saaa-capturing" | "saaa-transcribing" | "external-active" | "unknown";
  audioState: "silent" | "saaa-speaking" | "external-media" | "unknown";
}): Promise<void> {
  return invoke<void>("report_owned_signal", { input });
}

export async function submitSituationFeedback(input: {
  ledgerId: string;
  verdict: "accurate" | "inaccurate" | "unsure";
  impact: "none" | "no-effect" | "harmful";
  correctedScene: string | null;
  reasonCode: string | null;
}): Promise<SituationSnapshot> {
  return invoke<SituationSnapshot>("submit_situation_feedback", { input });
}

export const getSituationReviewSnapshot = (): Promise<SituationReviewSnapshot> => invoke("get_situation_review_snapshot");
export const createSituationCalibrationCandidate = (parameters: CalibrationParameters): Promise<CalibrationProfile> => invoke("create_situation_calibration_candidate", { parameters });
export const runSituationCalibration = (profileId: string): Promise<CalibrationRun> => invoke("run_situation_calibration", { profileId });
export const decideSituationCalibration = (profileId: string, decision: "accept" | "reject" | "rollback", reasonCode: string): Promise<SituationReviewSnapshot> => invoke("decide_situation_calibration", { profileId, decision, reasonCode });

export async function clearSituationHistory(): Promise<SituationSnapshot> {
  return invoke<SituationSnapshot>("clear_situation_history");
}

export async function meetingPreflight(input: { microphoneDeviceId: string; systemAudioEnabled: boolean; translationEnabled: boolean }): Promise<MeetingPreflightResult> { return invoke("meeting_preflight", { input }); }
export async function startMeeting(input: { sessionId: string; microphoneDeviceId: string; microphoneEnabled: boolean; systemAudioEnabled: boolean; translationEnabled: boolean; persistenceMode: "discard" }): Promise<MeetingSnapshot> { return invoke("start_meeting", { input }); }
export async function getMeetingSnapshot(): Promise<MeetingSnapshot> { return invoke("get_meeting_snapshot"); }
export async function watchMeeting(subscriberId: string, onEvent: (event: MeetingEvent) => void): Promise<void> { const channel = new Channel<MeetingEvent>(); channel.onmessage = onEvent; return invoke("watch_meeting", { subscriberId, onEvent: channel }); }
export async function unwatchMeeting(subscriberId: string): Promise<void> { return invoke("unwatch_meeting", { subscriberId }); }
export async function pauseMeeting(sessionId: string): Promise<MeetingSnapshot> { return invoke("pause_meeting", { input: { sessionId } }); }
export async function resumeMeeting(sessionId: string): Promise<MeetingSnapshot> { return invoke("resume_meeting", { input: { sessionId } }); }
export async function stopMeeting(sessionId: string): Promise<MeetingSnapshot> { return invoke("stop_meeting", { input: { sessionId } }); }
export async function appendMeetingAudioSegment(input: { sessionId: string; captureToken: string; lane: "microphone"; sequence: number; samples: Float32Array; sampleRate: number; startedAtMs: number; durationMs: number }): Promise<MeetingSegmentResult> { const { samples, ...metadata } = input; const audioUploadId = await stageAudioUpload(samples, "meeting-segment").finally(() => samples.fill(0)); return invoke("append_meeting_audio_segment", { input: { ...metadata, audioUploadId } }); }
export async function previewMeetingAudioSegment(input: { runId: string; sessionId: string; captureToken: string; lane: "microphone"; sequence: number; samples: Float32Array; sampleRate: number; startedAtMs: number; durationMs: number }): Promise<void> { const { samples, ...metadata } = input; const audioUploadId = await stageAudioUpload(samples, "meeting-segment").finally(() => samples.fill(0)); return invoke("preview_meeting_audio_segment", { input: { ...metadata, audioUploadId } }); }
export async function saveMeetingTranscript(sessionId: string): Promise<MeetingSnapshot> { return invoke("save_meeting_transcript", { input: { sessionId } }); }
export async function discardMeeting(sessionId: string): Promise<void> { return invoke("discard_meeting", { input: { sessionId } }); }
