import type { VoiceSettings } from "../../lib/contracts";

export function idleCaptureShouldStart({
  listeningEnabled,
  selectedConversationId,
  voiceSettings,
  situationHold,
  speechRunId,
  capture,
  hasStream,
  actionInProgress,
}: {
  listeningEnabled: boolean;
  selectedConversationId: string | null;
  voiceSettings: VoiceSettings | null;
  situationHold: boolean;
  speechRunId: string | null | undefined;
  capture: string;
  hasStream: boolean;
  actionInProgress: boolean;
}): boolean {
  return Boolean(
    listeningEnabled &&
    selectedConversationId &&
    voiceSettings &&
    !situationHold &&
    !speechRunId &&
    !actionInProgress &&
    capture === "idle" &&
    !hasStream,
  );
}
