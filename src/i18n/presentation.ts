import type { TFunction } from "i18next";
import type { ConversationRuntimeActivity } from "../lib/conversationActivity";

type ErrorScope = "app" | "chat" | "meeting" | "settings" | "situation" | "voice";

const messageKeys = {
  appPrimaryConversationUnavailable: "errors.app.primaryConversationUnavailable",
  appSurfaceSwitchBlocked: "app.surfaceSwitchBlocked",
  chatVoiceBlockedDuringMeeting: "errors.chat.voiceBlockedDuringMeeting",
  chatVoiceSettingsUnavailable: "errors.chat.voiceSettingsUnavailable",
  chatRecordedAudioUnavailable: "errors.chat.recordedAudioUnavailable",
  chatVoiceQueueFull: "errors.chat.voiceQueueFull",
  chatVoicePendingLimit: "errors.chat.voicePendingLimit",
  chatVoiceQueryQueued: "chat.activity.voiceQueryQueued",
  chatGenerationCancelled: "chat.activity.generationCancelled",
  chatSpeechPlaybackFailed: "errors.chat.speechPlaybackFailed",
  chatMicrophoneResumeFailed: "errors.chat.microphoneResumeFailed",
  chatVoiceCaptureInitializationFailed: "errors.chat.voiceCaptureInitializationFailed",
  chatVoiceAsrUnavailable: "errors.chat.voiceAsrUnavailable",
  chatVoiceSessionConflict: "errors.chat.voiceSessionConflict",
  chatVoiceTargetSpeakerModeUnavailable: "errors.chat.voiceTargetSpeakerModeUnavailable",
  voiceTargetSpeakerRejected: "errors.voice.targetSpeakerRejected",
  voiceAsrLanguageNotAllowed: "errors.voice.asrLanguageNotAllowed",
  voiceAsrLanguageUnknown: "errors.voice.asrLanguageUnknown",
  voiceAsrNoSpeech: "errors.voice.asrNoSpeech",
  voiceSamplePlaybackFailed: "errors.voice.samplePlaybackFailed",
  voiceProfileLoading: "voice.profile.loadingRuntime",
  voiceProfileNoAudio: "voice.profile.noAudio",
  voiceProfileMicrophoneTimeout: "errors.voice.microphoneStartupTimedOut",
  voiceProfileProcessorTimeout: "errors.voice.audioProcessorStartupTimedOut",
  meetingCaptureDisconnected: "meeting.health.captureDisconnected",
  meetingTranscriptionBackpressure: "errors.meeting.transcriptionBackpressure",
  meetingCaptureInactive: "errors.meeting.captureInactive",
  meetingVoiceSettingsUnavailable: "errors.meeting.voiceSettingsUnavailable",
  meetingStartFailed: "errors.meeting.startFailed",
  meetingRuntimeFailure: "errors.meeting.runtimeFailure",
  settingsAgentConnectionTokenInvalid: "settings.connection.agentConnectionTokenInvalid",
  settingsAgentConnectionAuthorizationRejected: "settings.connection.agentConnectionAuthorizationRejected",
} as const;

export type UiMessageName = keyof typeof messageKeys;
const MESSAGE_PREFIX = "saaa-ui:";

/**
 * Carries a stable UI message identity across non-React code. It is translated
 * only at the display boundary, so changing the display language updates an
 * already-visible error as well.
 */
export function uiMessage(name: UiMessageName): string {
  return `${MESSAGE_PREFIX}${name}`;
}

const legacyMessageNames: Record<string, UiMessageName> = {
  "Primary conversation is unavailable.": "appPrimaryConversationUnavailable",
  "Chat voice capture is disabled while a meeting is active or paused.": "chatVoiceBlockedDuringMeeting",
  "Voice settings are unavailable.": "chatVoiceSettingsUnavailable",
  "Recorded audio is unavailable.": "chatRecordedAudioUnavailable",
  "音声処理が追いつかないため、新しい発話は送信しませんでした。": "chatVoiceQueueFull",
  "応答待ちの音声クエリーが上限に達したため、新しい発話は送信しませんでした。": "chatVoicePendingLimit",
  "Voice query queued until the active response completes": "chatVoiceQueryQueued",
  "Generation cancelled": "chatGenerationCancelled",
  "サンプルを再生できませんでした。": "voiceSamplePlaybackFailed",
  "Meeting transcription cannot keep up. Capture was paused without evicting queued audio.": "meetingTranscriptionBackpressure",
  "Meeting capture is no longer active.": "meetingCaptureInactive",
  "capture disconnected — pause or stop to recover": "meetingCaptureDisconnected",
  "Loading local speaker verification…": "voiceProfileLoading",
  "Microphone startup timed out": "voiceProfileMicrophoneTimeout",
  "Audio processor startup timed out": "voiceProfileProcessorTimeout",
  "No audio was recorded.": "voiceProfileNoAudio",
  "音声が記録されませんでした。": "voiceProfileNoAudio",
  "LARM_API_TOKEN is invalid.": "settingsAgentConnectionTokenInvalid",
  "dynamic_lan rejected the connection authorization.": "settingsAgentConnectionAuthorizationRejected",
};

const microphoneMessageNames: Array<[RegExp, string]> = [
  [/secure application context/i, "errors.microphone.secureContextRequired"],
  [/capture is unavailable in this SAAA build/i, "errors.microphone.captureUnavailable"],
  [/device listing is unavailable/i, "errors.microphone.deviceListUnavailable"],
  [/access was denied/i, "errors.microphone.permissionDenied"],
  [/security policy/i, "errors.microphone.securityBlocked"],
  [/No microphone was found/i, "errors.microphone.deviceNotFound"],
  [/could not be opened/i, "errors.microphone.deviceUnavailable"],
  [/selected microphone is no longer available/i, "errors.microphone.deviceSelectionInvalid"],
  [/startup was interrupted/i, "errors.microphone.startupInterrupted"],
  [/audio processing could not start/i, "errors.microphone.processingCouldNotStart"],
  [/audio processing did not enter/i, "errors.microphone.processingDidNotStart"],
];

const statusKeys: Record<string, string> = {
  active: "common.active",
  available: "common.ready",
  busy: "situation.signalStates.busy",
  "capture-disconnected": "meeting.health.captureDisconnected",
  completed: "common.completed",
  configured: "common.configured",
  degraded: "common.degraded",
  disabled: "common.disabled",
  "external-active": "situation.signalStates.externalActive",
  "external-media": "situation.signalStates.externalMedia",
  failed: "common.failed",
  free: "situation.signalStates.free",
  inactive: "common.inactive",
  idle: "common.idle",
  "meeting-likely": "situation.signalStates.meetingLikely",
  missing: "common.missing",
  "model-running": "situation.signalStates.modelRunning",
  "agent-running": "situation.signalStates.agentRunning",
  "permission-denied": "common.permissionDenied",
  ready: "common.ready",
  recording: "chat.voiceStates.recording",
  recent: "common.recent",
  "saaa-capturing": "situation.signalStates.saaaCapturing",
  "saaa-speaking": "situation.signalStates.saaaSpeaking",
  "saaa-transcribing": "situation.signalStates.saaaTranscribing",
  silent: "situation.signalStates.silent",
  stopped: "meeting.health.stopped",
  stopping: "meeting.health.stopping",
  transcribing: "chat.voiceStates.transcribing",
  unavailable: "common.unavailable",
  unchecked: "common.unchecked",
  unknown: "common.unknown",
  unsupported: "common.unsupported",
  "user-input": "situation.signalStates.userInput",
};

const sceneKeys: Record<string, string> = {
  CONVERSATION: "conversation",
  MEETING: "meeting",
  CODING: "coding",
  WRITING: "writing",
  MEDIA: "media",
  FOCUS: "focus",
  SOLO: "solo",
  UNKNOWN: "unknown",
};

const reasonKeys: Record<string, string> = {
  "safe-default": "safeDefault",
  "input-idle": "inputIdle",
  "user-busy": "userBusy",
  "passive-observation": "passiveObservation",
  "low-confidence": "lowConfidence",
  "insufficient-evidence": "insufficientEvidence",
};

/** Localizes known runtime failures and hides untrusted backend/provider text. */
export function localizeUiMessage(t: TFunction, message: string | null | undefined, scope: ErrorScope): string {
  if (!message) return "";
  const trimmed = message.trim();
  const name = trimmed.startsWith(MESSAGE_PREFIX)
    ? trimmed.slice(MESSAGE_PREFIX.length) as UiMessageName
    : legacyMessageNames[trimmed];
  if (name && name in messageKeys) return t(messageKeys[name]);

  if (trimmed.startsWith("TARGET_SPEAKER_REJECTED")) return t(messageKeys.voiceTargetSpeakerRejected);
  if (trimmed.startsWith("ASR_LANGUAGE_NOT_ALLOWED")) return t(messageKeys.voiceAsrLanguageNotAllowed);
  if (trimmed.startsWith("ASR_LANGUAGE_UNKNOWN")) return t(messageKeys.voiceAsrLanguageUnknown);
  if (trimmed.startsWith("ASR_NO_SPEECH")) return t(messageKeys.voiceAsrNoSpeech);
  if (trimmed.startsWith("Speech playback failed:")) return t(messageKeys.chatSpeechPlaybackFailed);
  if (trimmed.startsWith("Microphone resume failed:")) return t(messageKeys.chatMicrophoneResumeFailed);
  if (trimmed.startsWith("Voice capture initialization failed:")) return t(messageKeys.chatVoiceCaptureInitializationFailed);
  if (trimmed.startsWith("Meeting start failed:")) return t(messageKeys.meetingStartFailed);

  const microphone = microphoneMessageNames.find(([pattern]) => pattern.test(trimmed));
  if (microphone) return t(microphone[1]);
  return t(`errors.${scope}.operationFailed`);
}

export function localizeStatus(t: TFunction, value: string | null | undefined): string {
  return value && statusKeys[value] ? t(statusKeys[value]) : t("common.unknown");
}

export function localizeSituationScene(t: TFunction, scene: string): string {
  return sceneKeys[scene] ? t(`situation.scenes.${sceneKeys[scene]}`) : t("common.unknown");
}

export function localizeForegroundCategory(t: TFunction, category: string): string {
  const known = ["communication", "coding", "writing", "browser", "media", "sensitive", "other", "unknown"];
  return known.includes(category) ? t(`situation.foregroundCategories.${category}`) : t("common.unknown");
}

export function localizeSituationReason(t: TFunction, reason: string): string {
  return reasonKeys[reason] ? t(`situation.reasons.${reasonKeys[reason]}`) : t("situation.reasons.unknown");
}

export function localizeSituationAttention(t: TFunction, attention: string): string {
  return attention === "available" || attention === "busy" || attention === "unknown"
    ? t(`situation.attentionValueStates.${attention}`)
    : t("common.unknown");
}

export function localizeSituationEntryKind(t: TFunction, entryKind: string): string {
  return entryKind === "transition" || entryKind === "decision" || entryKind === "heartbeat"
    ? t(`situation.entryKinds.${entryKind}`)
    : t("common.unknown");
}

export function localizeMeetingLane(t: TFunction, lane: string): string {
  return lane === "microphone" || lane === "system-audio"
    ? t(`meeting.lanes.${lane}`)
    : t("common.unknown");
}

export function localizeProviderKind(t: TFunction, kind: string): string {
  const known = ["openai-compatible", "cloud-asr", "cloud-tts", "system-tts", "larm", "dynamic-lan"];
  return known.includes(kind) ? t(`settings.providers.kinds.${kind}`) : t("common.unknown");
}

export function localizeProviderLabel(t: TFunction, label: string): string {
  if (label === "Model not selected") return t("chat.processingNotSelected");
  if (label === "Provider Harness LLM") return t("settings.providers.defaultLabels.harnessLlm");
  if (label === "System Voice") return t("settings.providers.defaultLabels.systemVoice");
  const cloud = /^(?:Cloud |クラウド)(LLM|ASR|TTS)$/.exec(label);
  return cloud ? t("settings.providers.defaultName", { capability: cloud[1] }) : label;
}

/** Localizes a typed, presentation-safe runtime activity. */
export function localizeRuntimeActivity(t: TFunction, activity: ConversationRuntimeActivity): string {
  switch (activity.type) {
    case "providerStarted":
      return t("chat.activity.usingProvider", { provider: activity.providerId });
    case "providerSelected":
      return t(activity.fallbackUsed ? "chat.activity.usingFallbackProvider" : "chat.activity.usingProvider", { provider: activity.providerId });
    case "providerWorking":
      return t("chat.activity.working");
    case "providerFailed":
      return t("chat.activity.providerFailed");
    case "generationCancelled":
      return t("chat.activity.generationCancelled");
    case "voiceQueryQueued":
      return t("chat.activity.voiceQueryQueued");
  }
}

export function formatRegionalDateTime(value: string, language: string | undefined, timeZone: string): string {
  const milliseconds = Number(value);
  if (!Number.isFinite(milliseconds)) return value;
  return new Date(milliseconds).toLocaleString(language, timeZone === "system" ? undefined : { timeZone });
}
