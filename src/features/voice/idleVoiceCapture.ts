import type { VoiceSettings } from "../../lib/contracts";

export function idleCaptureShouldStart({
  listeningEnabled,
  selectedConversationId,
  voiceSettings,
  situationHold,
  speechRunId,
  capture,
  hasStream,
}: {
  listeningEnabled: boolean;
  selectedConversationId: string | null;
  voiceSettings: VoiceSettings | null;
  situationHold: boolean;
  speechRunId: string | null | undefined;
  capture: string;
  hasStream: boolean;
}): boolean {
  return Boolean(
    listeningEnabled &&
    selectedConversationId &&
    voiceSettings &&
    !situationHold &&
    !speechRunId &&
    capture === "idle" &&
    !hasStream,
  );
}
