import type { VoiceSettings, ConversationVoicePolicySnapshot } from "../../lib/contracts";
import { toMessage } from "../../lib/appHelpers";
import { uiMessage } from "../../i18n/presentation";
import { MicrophoneCaptureError, microphoneErrorMessage } from "../../lib/microphone";

export function voiceStartupMessage(cause: unknown): string {
  if (cause instanceof MicrophoneCaptureError) return microphoneErrorMessage(cause);
  switch (toMessage(cause)) {
    case "asr-provider-unavailable":
      return uiMessage("chatVoiceAsrUnavailable");
    case "asr-session-exists":
      return uiMessage("chatVoiceSessionConflict");
    case "asr-target-speaker-unavailable":
      return uiMessage("chatVoiceTargetSpeakerModeUnavailable");
    default:
      return uiMessage("chatVoiceCaptureInitializationFailed");
  }
}

export function effectiveCaptureSettings(
  settings: VoiceSettings | null,
  policy: ConversationVoicePolicySnapshot | null,
): VoiceSettings | null {
  if (!settings) return null;
  return policy ? { ...settings, silenceTimeoutMs: policy.effectiveSilenceTimeoutMs } : settings;
}

export function captureAvailability(
  listening: boolean,
  capture: import("../../lib/voiceSession").VoiceSession["capture"],
) {
  if (!listening) return "disabled" as const;
  return (
    {
      starting: "connecting",
      recording: "listening",
      suspended: "suspended",
      idle: "blocked",
    } as const
  )[capture];
}
