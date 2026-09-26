import type { VoiceSettings } from "../../lib/contracts";

export function idleCaptureShouldStart({
  listeningEnabled,
  selectedConversationId,
  voiceSettings,
  capture,
  hasStream,
  actionInProgress,
}: {
  listeningEnabled: boolean;
  selectedConversationId: string | null;
  voiceSettings: VoiceSettings | null;
  capture: string;
  hasStream: boolean;
  actionInProgress: boolean;
}): boolean {
  return Boolean(
    listeningEnabled &&
    selectedConversationId &&
    voiceSettings &&
    !actionInProgress &&
    capture === "idle" &&
    !hasStream,
  );
}
