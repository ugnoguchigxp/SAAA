import type { VoiceSettings } from "../../lib/contracts";

export function idleCaptureShouldStart({
  listeningEnabled,
  selectedConversationId,
  voiceSettings,
  meetingBlocked,
  speechRunId,
  capture,
  hasStream,
}: {
  listeningEnabled: boolean;
  selectedConversationId: string | null;
  voiceSettings: VoiceSettings | null;
  meetingBlocked: boolean;
  speechRunId: string | null | undefined;
  capture: string;
  hasStream: boolean;
}): boolean {
  return Boolean(
    listeningEnabled &&
      selectedConversationId &&
      voiceSettings &&
      !meetingBlocked &&
      !speechRunId &&
      capture === "idle" &&
      !hasStream,
  );
}
