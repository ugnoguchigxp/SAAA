import type { VoiceSettings, ConversationVoicePolicySnapshot } from "../../lib/contracts";
import { toMessage } from "../../lib/appHelpers";
import { uiMessage } from "../../i18n/presentation";
import { MicrophoneCaptureError, microphoneErrorMessage } from "../../lib/microphone";

export function voiceStartupMessage(cause: unknown): string {
  if (cause instanceof MicrophoneCaptureError) return microphoneErrorMessage(cause);
  if (toMessage(cause).startsWith("lfm-")) return `LFMを準備できません: ${toMessage(cause)}`;
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
  // A pause submits a conversational segment to LFM, not a completed request to Qwen.
  void policy;
  return { ...settings, silenceTimeoutMs: 1_500 };
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
